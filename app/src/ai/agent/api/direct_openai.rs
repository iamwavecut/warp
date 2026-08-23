use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use anyhow::Context as _;
use async_stream::stream;
use base64::Engine as _;
use chrono::{DateTime, Local};
use futures_util::StreamExt as _;
use prost::Message as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use warp_multi_agent_api as api;

use crate::ai::agent::{
    AIAgentActionResult, AIAgentActionResultType, AIAgentAttachment, AIAgentContext, AIAgentInput,
    AnyFileContent, DriveObjectPayload, MarkdownActionResult, UserQueryMode,
};
use crate::ai::llms::LLMId;
use crate::server::server_api::AIApiError;
use crate::settings::{
    CustomProviderCapabilities, CustomProviderConfig, custom_provider_name_is_unique,
    normalize_custom_provider_env_var,
};
use ::ai::api_keys::ApiKeys;

use super::ResponseStream;
use crate::ai::block_context::BlockContext;

const CUSTOM_MODEL_PREFIX: &str = "custom/";
const MAX_CONTEXT_CHARS: usize = 24_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CustomModelId {
    pub provider_name: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomProviderRoute {
    pub provider_name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub capabilities: CustomProviderCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CustomProviderRouteError {
    AmbiguousProviderName(String),
    KeysNotReady,
}

impl std::fmt::Display for CustomProviderRouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AmbiguousProviderName(name) => write!(
                f,
                "custom provider name `{name}` is ambiguous; rename one provider before using it"
            ),
            Self::KeysNotReady => write!(
                f,
                "Local API keys are still loading; retry the request when secure storage is ready."
            ),
        }
    }
}

impl std::error::Error for CustomProviderRouteError {}

/// Capabilities that the current direct OpenAI-compatible adapter can actually
/// deliver. Configured vision, embeddings, and transcription remain disabled
/// until their local adapters are implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectiveCustomProviderCapabilities {
    pub chat: bool,
    pub tools: bool,
    pub vision: bool,
    pub embeddings: bool,
    pub transcription: bool,
}

impl CustomProviderRoute {
    pub(crate) fn effective_capabilities(&self) -> EffectiveCustomProviderCapabilities {
        effective_capabilities_for_config(&self.capabilities)
    }

    /// Convert configured model tokens to a character budget for context
    /// payloads. This is not whole-request token accounting and reserves no
    /// output tokens; exact accounting belongs to the future compaction
    /// boundary. The legacy fixed budget remains the fallback.
    pub(crate) fn context_char_budget(&self) -> usize {
        self.capabilities
            .context_window_tokens
            .map(|tokens| (tokens as usize).saturating_mul(3))
            .unwrap_or(MAX_CONTEXT_CHARS)
    }
}

pub(crate) fn effective_capabilities_for_config(
    capabilities: &CustomProviderCapabilities,
) -> EffectiveCustomProviderCapabilities {
    EffectiveCustomProviderCapabilities {
        chat: capabilities.chat,
        tools: capabilities.tools,
        vision: false,
        embeddings: false,
        transcription: false,
    }
}

pub(super) fn parse_custom_model_id(model_id: &str) -> Option<CustomModelId> {
    let remainder = model_id.strip_prefix(CUSTOM_MODEL_PREFIX)?;
    let (provider_name, model) = remainder.split_once('/')?;
    if provider_name.trim().is_empty() || model.trim().is_empty() {
        return None;
    }

    Some(CustomModelId {
        provider_name: provider_name.to_string(),
        model: model.to_string(),
    })
}

pub(super) fn is_custom_model_id(model_id: &LLMId) -> bool {
    model_id.as_str().starts_with(CUSTOM_MODEL_PREFIX)
}

pub(crate) fn resolve_custom_provider_route(
    model_id: &str,
    providers: &[CustomProviderConfig],
    api_keys: &ApiKeys,
) -> Option<CustomProviderRoute> {
    resolve_custom_provider_route_with_error(model_id, providers, api_keys)
        .ok()
        .flatten()
}

pub(crate) fn resolve_custom_provider_route_with_error(
    model_id: &str,
    providers: &[CustomProviderConfig],
    api_keys: &ApiKeys,
) -> Result<Option<CustomProviderRoute>, CustomProviderRouteError> {
    resolve_custom_provider_route_with_readiness(model_id, providers, api_keys, true)
}

pub(crate) fn resolve_custom_provider_route_with_readiness(
    model_id: &str,
    providers: &[CustomProviderConfig],
    api_keys: &ApiKeys,
    keys_ready: bool,
) -> Result<Option<CustomProviderRoute>, CustomProviderRouteError> {
    let Some(custom_model) = parse_custom_model_id(model_id) else {
        return Ok(None);
    };
    if !keys_ready {
        return Err(CustomProviderRouteError::KeysNotReady);
    }
    if !custom_provider_name_is_unique(&custom_model.provider_name, providers) {
        let provider_name = custom_model.provider_name.clone();
        let has_matching_provider = providers
            .iter()
            .any(|provider| provider.name == provider_name);
        return if has_matching_provider {
            Err(CustomProviderRouteError::AmbiguousProviderName(
                provider_name,
            ))
        } else {
            Ok(None)
        };
    }
    let Some(provider) = providers.iter().find(|provider| {
        provider.name == custom_model.provider_name && provider.validate().is_ok()
    }) else {
        return Ok(None);
    };

    Ok(Some(route_for_provider_model(
        provider,
        custom_model.model,
        api_keys,
    )))
}

pub(crate) fn default_custom_provider_route(
    providers: &[CustomProviderConfig],
    api_keys: &ApiKeys,
) -> Option<CustomProviderRoute> {
    default_custom_provider_route_with_error(providers, api_keys, true)
        .ok()
        .flatten()
}

pub(crate) fn default_custom_provider_route_when_ready(
    providers: &[CustomProviderConfig],
    api_keys: &ApiKeys,
    keys_ready: bool,
) -> Option<CustomProviderRoute> {
    default_custom_provider_route_with_error(providers, api_keys, keys_ready)
        .ok()
        .flatten()
}

pub(crate) fn default_custom_provider_route_with_error(
    providers: &[CustomProviderConfig],
    api_keys: &ApiKeys,
    keys_ready: bool,
) -> Result<Option<CustomProviderRoute>, CustomProviderRouteError> {
    if !keys_ready {
        return Err(CustomProviderRouteError::KeysNotReady);
    }

    if let Some(provider) = providers
        .iter()
        .find(|provider| !custom_provider_name_is_unique(&provider.name, providers))
    {
        return Err(CustomProviderRouteError::AmbiguousProviderName(
            provider.name.clone(),
        ));
    }

    for provider in providers {
        if provider.validate().is_err() {
            continue;
        }
        let Some(model) = provider.models.first().cloned() else {
            continue;
        };
        return Ok(Some(route_for_provider_model(provider, model, api_keys)));
    }
    Ok(None)
}

fn route_for_provider_model(
    provider: &CustomProviderConfig,
    model: String,
    api_keys: &ApiKeys,
) -> CustomProviderRoute {
    let secure_storage_key = api_keys.custom.get(&provider.name).cloned();
    let env_key = provider
        .api_key_env_var
        .as_deref()
        .and_then(normalize_custom_provider_env_var)
        .and_then(|env_var| std::env::var(env_var).ok());

    CustomProviderRoute {
        provider_name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        model,
        api_key: secure_storage_key.or(env_key),
        capabilities: provider.capabilities.clone(),
    }
}

pub(super) fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

pub(crate) fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAITool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct ChatMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    tool_calls: Vec<OpenAIToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl ChatMessage {
    fn system(content: String) -> Self {
        Self {
            role: "system",
            content: Some(content),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    fn user(content: String) -> Self {
        Self {
            role: "user",
            content: Some(content),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    fn assistant(content: String) -> Self {
        Self {
            role: "assistant",
            content: Some(content),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    fn assistant_tool_call(tool_call: OpenAIToolCall) -> Self {
        Self::assistant_tool_calls(vec![tool_call])
    }

    fn assistant_tool_calls(tool_calls: Vec<OpenAIToolCall>) -> Self {
        Self {
            role: "assistant",
            content: None,
            tool_calls,
            tool_call_id: None,
        }
    }

    fn tool(tool_call_id: String, content: String) -> Self {
        Self {
            role: "tool",
            content: Some(content),
            tool_calls: vec![],
            tool_call_id: Some(tool_call_id),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAIToolFunction,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAIToolFunction {
    name: &'static str,
    description: &'static str,
    parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAIToolCall>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChunkChoice {
    delta: ChatChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatChunkDelta {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamingToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct StreamingToolCallDelta {
    index: usize,
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: Option<StreamingFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct StreamingFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct StreamingToolCall {
    seen: bool,
    id: Option<String>,
    kind: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl StreamingToolCall {
    fn apply_delta(&mut self, delta: StreamingToolCallDelta) {
        self.seen = true;
        if let Some(id) = delta.id {
            self.id = Some(id);
        }
        if let Some(kind) = delta.kind {
            self.kind = Some(kind);
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                self.name = Some(name);
            }
            if let Some(arguments) = function.arguments {
                self.arguments.push_str(&arguments);
            }
        }
    }

    fn finish(self) -> Result<OpenAIToolCall, AIApiError> {
        let id = self.id.ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible tool call was missing a non-empty id"
            ))
        })?;
        if id.trim().is_empty() {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible tool call was missing a non-empty id"
            )));
        }
        let kind = self.kind.ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible tool call was missing type `function`"
            ))
        })?;
        if kind != "function" {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible tool call type must be exactly `function`, got `{kind}`"
            )));
        }
        let name = self.name.ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible tool call was missing a function name"
            ))
        })?;
        if name.trim().is_empty() {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible tool call was missing a non-empty function name"
            )));
        }
        let tool_call = OpenAIToolCall {
            id,
            kind,
            function: OpenAIFunctionCall {
                name,
                arguments: self.arguments,
            },
        };
        validate_openai_tool_call_envelope(&tool_call).map(|_| tool_call)
    }
}

#[derive(Default)]
struct StreamCompletionState {
    content_message_id: Option<String>,
    tool_calls: Vec<StreamingToolCall>,
    finish_reason: Option<String>,
    content_chars: usize,
    parsed_events: usize,
}

impl StreamCompletionState {
    fn apply_chunk(
        &mut self,
        chunk: ChatCompletionChunk,
        task_id: &str,
        request_id: &str,
        prefix_actions: &mut Vec<api::ClientAction>,
    ) -> Vec<api::ResponseEvent> {
        let mut events = Vec::new();
        for choice in chunk.choices {
            if let Some(reason) = choice.finish_reason.filter(|reason| !reason.is_empty()) {
                self.finish_reason = Some(reason);
            }

            if let Some(delta) = choice.delta.content.filter(|text| !text.is_empty()) {
                self.content_chars += delta.len();
                if let Some(message_id) = &self.content_message_id {
                    events.push(append_agent_output_event(task_id, message_id, delta));
                } else {
                    let message_id = Uuid::new_v4().to_string();
                    let mut actions = take_prefix_actions(prefix_actions);
                    actions.push(add_messages_action(
                        task_id,
                        vec![agent_output_message(
                            task_id,
                            request_id,
                            message_id.clone(),
                            delta,
                        )],
                    ));
                    events.push(client_actions_event(actions));
                    self.content_message_id = Some(message_id);
                }
            }

            for tool_delta in choice.delta.tool_calls {
                if self.tool_calls.len() <= tool_delta.index {
                    self.tool_calls
                        .resize_with(tool_delta.index + 1, StreamingToolCall::default);
                }
                self.tool_calls[tool_delta.index].apply_delta(tool_delta);
            }
        }
        events
    }
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: Option<String>,
}

pub(crate) async fn fetch_models(
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, AIApiError> {
    let client = reqwest::Client::new();
    let mut request = client.get(models_url(base_url));
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|e| format!("(failed to read response body: {e:#})"));
        return Err(AIApiError::ErrorStatus(status, body));
    }

    let response: ModelsResponse = response
        .json()
        .await
        .context("failed to decode OpenAI-compatible models response")?;
    let mut seen = std::collections::HashSet::new();
    let models = response
        .data
        .into_iter()
        .filter_map(|entry| entry.id)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .filter(|id| seen.insert(id.clone()))
        .collect();

    Ok(models)
}

pub(crate) async fn complete_text(
    route: CustomProviderRoute,
    system_prompt: String,
    user_prompt: String,
) -> Result<String, AIApiError> {
    if !route.effective_capabilities().chat {
        return Err(AIApiError::Other(anyhow::anyhow!(chat_disabled_message(
            &route.provider_name
        ))));
    }

    let body = ChatCompletionRequest {
        model: route.model.clone(),
        messages: vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(user_prompt),
        ],
        stream: false,
        tools: vec![],
        tool_choice: None,
        parallel_tool_calls: None,
    };

    let response = send_chat_completion_request(&route, &body).await?;
    let response: ChatCompletionResponse = response
        .json()
        .await
        .context("failed to decode OpenAI-compatible chat completion response")?;
    let content = response
        .choices
        .into_iter()
        .find_map(|choice| choice.message.content)
        .unwrap_or_default()
        .trim()
        .to_string();

    if content.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible provider returned an empty completion"
        )));
    }

    Ok(content)
}

fn create_task_action(task_id: &str) -> api::ClientAction {
    api::ClientAction {
        action: Some(api::client_action::Action::CreateTask(
            api::client_action::CreateTask {
                task: Some(api::Task {
                    id: task_id.to_string(),
                    description: String::new(),
                    dependencies: None,
                    messages: vec![],
                    summary: String::new(),
                    server_data: String::new(),
                }),
            },
        )),
    }
}

#[cfg(test)]
fn create_task_event(task_id: &str) -> api::ResponseEvent {
    client_actions_event(vec![create_task_action(task_id)])
}

fn response_task_id(params: &super::RequestParams) -> String {
    params
        .request_task_id
        .clone()
        .or_else(|| params.tasks.first().map(|task| task.id.clone()))
        .unwrap_or_else(|| "local-root-task".to_string())
}

fn should_create_task(params: &super::RequestParams, task_id: &str) -> bool {
    !params.tasks.iter().any(|task| task.id == task_id)
}

pub(super) async fn generate(
    route: CustomProviderRoute,
    params: super::RequestParams,
    supported_tools: Vec<api::ToolType>,
) -> Result<ResponseStream, super::ConvertToAPITypeError> {
    let effective_capabilities = route.effective_capabilities();
    if !effective_capabilities.chat {
        return Ok(error_stream(chat_disabled_message(&route.provider_name)));
    }
    if !effective_capabilities.vision && input_contains_image(&params.input) {
        return Ok(error_stream(vision_disabled_message(&route.provider_name)));
    }

    let task_id = response_task_id(&params);
    let needs_create_task = should_create_task(&params, &task_id);
    let request_id = Uuid::new_v4().to_string();
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let tools = if effective_capabilities.tools {
        openai_tools_for_supported_tools(&supported_tools)
    } else {
        Vec::new()
    };
    let advertised_supported_tools = if effective_capabilities.tools {
        supported_tools.clone()
    } else {
        Vec::new()
    };
    let long_running_shell_controls_advertised =
        supports_complete_long_running_shell_family(&advertised_supported_tools);
    let input_messages = api_messages_from_inputs(&task_id, &request_id, &params.input);
    let chat_messages = openai_messages_from_params_with_tool_policy(
        &params,
        &tools,
        route.context_char_budget(),
        long_running_shell_controls_advertised,
    );
    log::info!(
        "Using OpenAI-compatible custom provider route: provider={}, model={}, advertised_tools={}, task_count={}, input_count={}, chat_message_count={}",
        route.provider_name,
        route.model,
        tools.len(),
        params.tasks.len(),
        params.input.len(),
        chat_messages.len()
    );

    let output_stream = stream! {
        yield Ok(api::ResponseEvent {
            r#type: Some(api::response_event::Type::Init(api::response_event::StreamInit {
                conversation_id: conversation_id.clone(),
                request_id: request_id.clone(),
                run_id: String::new(),
            })),
        });

        let mut prefix_actions = Vec::new();
        if needs_create_task {
            prefix_actions.push(create_task_action(&task_id));
        }
        if !input_messages.is_empty() {
            prefix_actions.push(add_messages_action(&task_id, input_messages));
        }

        let body = ChatCompletionRequest {
            model: route.model.clone(),
            messages: chat_messages,
            stream: true,
            tool_choice: (!tools.is_empty()).then_some("auto"),
            parallel_tool_calls: (!tools.is_empty()).then_some(true),
            tools,
        };

        let mut completion_events = stream_chat_completion_with_tool_policy(
            route.clone(),
            body,
            task_id.clone(),
            request_id.clone(),
            prefix_actions,
            advertised_supported_tools.clone(),
            long_running_shell_controls_advertised,
        );
        while let Some(event) = completion_events.next().await {
            match event {
                Ok(event) => yield Ok(event),
                Err(error) => {
                    yield Err(error);
                    return;
                }
            }
        }
    };

    Ok(Box::pin(output_stream))
}

fn chat_disabled_message(provider_name: &str) -> String {
    format!(
        "Custom provider `{provider_name}` has chat disabled in its local capability configuration; enable chat or choose another configured model."
    )
}

fn vision_disabled_message(provider_name: &str) -> String {
    format!(
        "Custom provider `{provider_name}` received image context, but vision is not implemented by the local adapter yet; choose a text-only request or another local model."
    )
}

fn input_contains_image(input: &[AIAgentInput]) -> bool {
    input.iter().any(|item| {
        let context = match item {
            AIAgentInput::UserQuery { context, .. }
            | AIAgentInput::AutoCodeDiffQuery { context, .. }
            | AIAgentInput::ResumeConversation { context }
            | AIAgentInput::InitProjectRules { context, .. }
            | AIAgentInput::CreateEnvironment { context, .. }
            | AIAgentInput::CreateNewProject { context, .. }
            | AIAgentInput::CloneRepository { context, .. }
            | AIAgentInput::CodeReview { context, .. }
            | AIAgentInput::SummarizeConversation { context, .. }
            | AIAgentInput::InvokeSkill { context, .. }
            | AIAgentInput::StartFromAmbientRunPrompt { context, .. }
            | AIAgentInput::ActionResult { context, .. }
            | AIAgentInput::PassiveSuggestionResult { context, .. } => Some(context),
            AIAgentInput::TriggerPassiveSuggestion { .. }
            | AIAgentInput::MessagesReceivedFromAgents { .. }
            | AIAgentInput::EventsFromAgents { .. }
            | AIAgentInput::OrchestrationConfigUpdate { .. } => None,
        };
        context.is_some_and(|context| {
            context
                .iter()
                .any(|item| matches!(item, AIAgentContext::Image(_)))
        })
    })
}

fn stream_chat_completion(
    route: CustomProviderRoute,
    body: ChatCompletionRequest,
    task_id: String,
    request_id: String,
    prefix_actions: Vec<api::ClientAction>,
) -> ResponseStream {
    stream_chat_completion_with_tool_policy(
        route,
        body,
        task_id,
        request_id,
        prefix_actions,
        vec![api::ToolType::RunShellCommand],
        false,
    )
}

fn stream_chat_completion_with_tool_policy(
    route: CustomProviderRoute,
    body: ChatCompletionRequest,
    task_id: String,
    request_id: String,
    mut prefix_actions: Vec<api::ClientAction>,
    supported_tools: Vec<api::ToolType>,
    long_running_shell_controls_advertised: bool,
) -> ResponseStream {
    Box::pin(stream! {
    let response = match send_chat_completion_request(&route, &body).await {
        Ok(response) => response,
        Err(error) => {
            yield Err(Arc::new(error));
            return;
        }
    };
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if !content_type.contains("text/event-stream") {
        let response: ChatCompletionResponse = match response
            .json()
            .await
            .context("failed to decode OpenAI-compatible chat completion response")
        {
            Ok(response) => response,
            Err(error) => {
                yield Err(Arc::new(AIApiError::Other(error)));
                return;
            }
        };
        match events_from_non_streaming_response(
            response,
            &task_id,
            &request_id,
            prefix_actions,
            &supported_tools,
            long_running_shell_controls_advertised,
        ) {
            Ok(events) => {
                for event in events {
                    yield Ok(event);
                }
            }
            Err(error) => yield Err(Arc::new(error)),
        }
        return;
    }

    let mut bytes = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut state = StreamCompletionState::default();

    while let Some(chunk) = bytes.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                let error: AIApiError = error.into();
                yield Err(Arc::new(error));
                return;
            }
        };
        buffer.extend_from_slice(&chunk);

        while let Some((event_end, delimiter_len)) = sse_event_end(&buffer) {
            let event_bytes = buffer[..event_end].to_vec();
            buffer.drain(..event_end + delimiter_len);
            let event = match String::from_utf8(event_bytes) {
                Ok(event) => event,
                Err(error) => {
                    yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(
                        "malformed OpenAI-compatible SSE event is not valid UTF-8: {error}"
                    ))));
                    return;
                }
            };

            let events = match apply_openai_sse_event(
                &event,
                &mut state,
                &task_id,
                &request_id,
                &mut prefix_actions,
            ) {
                Ok(events) => events,
                Err(error) => {
                    yield Err(Arc::new(error));
                    return;
                }
            };
            for event in events {
                yield Ok(event);
            }
        }
    }

    let residual_buffer_bytes = buffer.len();
    if !buffer.is_empty() && !buffer.iter().all(u8::is_ascii_whitespace) {
        let residual_event = match String::from_utf8(std::mem::take(&mut buffer)) {
            Ok(event) => event,
            Err(error) => {
                yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(
                    "malformed OpenAI-compatible SSE event is not valid UTF-8: {error}"
                ))));
                return;
            }
        };
        let events = match apply_openai_sse_event(
            &residual_event,
            &mut state,
            &task_id,
            &request_id,
            &mut prefix_actions,
        ) {
            Ok(events) => events,
            Err(error) => {
                yield Err(Arc::new(error));
                return;
            }
        };
        for event in events {
            yield Ok(event);
        }
    }

    let StreamCompletionState {
        tool_calls,
        finish_reason,
        content_chars,
        parsed_events,
        ..
    } = state;
    let Some(last_tool_call_index) = tool_calls.iter().rposition(|tool_call| tool_call.seen)
    else {
        if finish_reason.as_deref() == Some("tool_calls") {
            yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(
                "malformed OpenAI-compatible stream ended with tool_calls but no tool call"
            ))));
            return;
        }
        log::info!(
            "OpenAI-compatible stream finished: request_id={}, finish_reason={:?}, content_chars={}, tool_calls={}, parsed_events={}, residual_buffer_bytes={}",
            request_id,
            finish_reason,
            content_chars,
            0,
            parsed_events,
            residual_buffer_bytes
        );
        if !prefix_actions.is_empty() {
            yield Ok(client_actions_event(take_prefix_actions(&mut prefix_actions)));
        }
        yield Ok(finished_event_for_openai_finish_reason(
            finish_reason.as_deref(),
            0,
        ));
        return;
    };
    if tool_calls[..=last_tool_call_index]
        .iter()
        .any(|tool_call| !tool_call.seen)
    {
        yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(
            "malformed OpenAI-compatible stream omitted a tool call index"
        ))));
        return;
    }
    let completed_tool_calls = match tool_calls
        .into_iter()
        .take(last_tool_call_index + 1)
        .map(StreamingToolCall::finish)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(tool_calls) => tool_calls,
        Err(error) => {
            yield Err(Arc::new(error));
            return;
        }
    };
    let completed_tool_call_count = completed_tool_calls.len();
    log::info!(
        "OpenAI-compatible stream finished: request_id={}, finish_reason={:?}, content_chars={}, tool_calls={}, parsed_events={}, residual_buffer_bytes={}",
        request_id,
        finish_reason,
        content_chars,
        completed_tool_call_count,
        parsed_events,
        residual_buffer_bytes
    );
    if !completed_tool_calls.is_empty() {
        let mut messages = Vec::new();
        for tool_call in completed_tool_calls {
            match api_tool_call_message_for_supported_tools(
                &task_id,
                &request_id,
                tool_call,
                &supported_tools,
                long_running_shell_controls_advertised,
            ) {
                Ok(message) => messages.push(message),
                Err(error) => {
                    yield Err(Arc::new(error));
                    return;
                }
            }
        }
        let mut actions = take_prefix_actions(&mut prefix_actions);
        actions.push(add_messages_action(&task_id, messages));
        yield Ok(client_actions_event(actions));
    } else if !prefix_actions.is_empty() {
        yield Ok(client_actions_event(take_prefix_actions(&mut prefix_actions)));
    }
    yield Ok(finished_event_for_openai_finish_reason(
        finish_reason.as_deref(),
        completed_tool_call_count,
    ));
    })
}

async fn send_chat_completion_request(
    route: &CustomProviderRoute,
    body: &ChatCompletionRequest,
) -> Result<reqwest::Response, AIApiError> {
    let client = reqwest::Client::new();
    let mut request = client
        .post(chat_completions_url(&route.base_url))
        .json(body);
    if let Some(api_key) = route.api_key.as_ref().filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|e| format!("(failed to read response body: {e:#})"));
        return Err(AIApiError::ErrorStatus(status, body));
    }

    Ok(response)
}

fn events_from_non_streaming_response(
    response: ChatCompletionResponse,
    task_id: &str,
    request_id: &str,
    mut prefix_actions: Vec<api::ClientAction>,
    supported_tools: &[api::ToolType],
    long_running_shell_controls_advertised: bool,
) -> Result<Vec<api::ResponseEvent>, AIApiError> {
    let mut events = Vec::new();
    let Some(choice) = response.choices.into_iter().next() else {
        if !prefix_actions.is_empty() {
            events.push(client_actions_event(prefix_actions));
        }
        events.push(finished_event_for_openai_finish_reason(None, 0));
        return Ok(events);
    };
    let ChatChoice {
        message,
        finish_reason,
    } = choice;
    let tool_call_count = message.tool_calls.len();
    let content_chars = message.content.as_ref().map(|text| text.len()).unwrap_or(0);

    let mut messages = Vec::new();
    if let Some(text) = message.content.filter(|text| !text.is_empty()) {
        messages.push(agent_output_message(
            task_id,
            request_id,
            Uuid::new_v4().to_string(),
            text,
        ));
    }
    for tool_call in message.tool_calls {
        messages.push(api_tool_call_message_for_supported_tools(
            task_id,
            request_id,
            tool_call,
            supported_tools,
            long_running_shell_controls_advertised,
        )?);
    }

    let mut actions = take_prefix_actions(&mut prefix_actions);
    if !messages.is_empty() {
        actions.push(add_messages_action(task_id, messages));
    }
    if !actions.is_empty() {
        events.push(client_actions_event(actions));
    }
    log::info!(
        "OpenAI-compatible non-stream response finished: request_id={}, finish_reason={:?}, content_chars={}, tool_calls={}",
        request_id,
        finish_reason,
        content_chars,
        tool_call_count
    );
    events.push(finished_event_for_openai_finish_reason(
        finish_reason.as_deref(),
        tool_call_count,
    ));

    Ok(events)
}

fn sse_event_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(lf), Some(crlf)) if lf <= crlf => Some((lf, 2)),
        (Some(lf), _) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn sse_data_payloads(event: &str) -> Result<Vec<String>, AIApiError> {
    let mut payloads = Vec::new();
    for line in event.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            payloads.push(data.trim_start().to_string());
        } else if !line.trim().is_empty() {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "malformed OpenAI-compatible SSE event field `{line}`"
            )));
        }
    }
    if payloads.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "malformed OpenAI-compatible SSE event: missing data field"
        )));
    }
    Ok(payloads)
}

fn apply_openai_sse_event(
    event: &str,
    state: &mut StreamCompletionState,
    task_id: &str,
    request_id: &str,
    prefix_actions: &mut Vec<api::ClientAction>,
) -> Result<Vec<api::ResponseEvent>, AIApiError> {
    let mut events = Vec::new();
    for data in sse_data_payloads(event)? {
        if data.trim() == "[DONE]" {
            continue;
        }

        state.parsed_events += 1;
        let chunk: ChatCompletionChunk = serde_json::from_str(&data).with_context(|| {
            format!("failed to decode OpenAI-compatible SSE event JSON: {data}")
        })?;
        events.extend(state.apply_chunk(chunk, task_id, request_id, prefix_actions));
    }
    Ok(events)
}

fn take_prefix_actions(prefix_actions: &mut Vec<api::ClientAction>) -> Vec<api::ClientAction> {
    std::mem::take(prefix_actions)
}

/// Builds the direct request messages while bounding context payloads at the
/// route-derived character budget. System/task messages are not token-counted;
/// this is intentionally a narrow truncation boundary until compaction has
/// exact model-token accounting.
fn openai_messages_from_params(
    params: &super::RequestParams,
    tools: &[OpenAITool],
    context_char_budget: usize,
) -> Vec<ChatMessage> {
    openai_messages_from_params_with_tool_policy(params, tools, context_char_budget, false)
}

fn openai_messages_from_params_with_tool_policy(
    params: &super::RequestParams,
    tools: &[OpenAITool],
    context_char_budget: usize,
    long_running_shell_controls_advertised: bool,
) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage::system(system_prompt(
        params,
        tools,
        context_char_budget,
    ))];

    for task in &params.tasks {
        messages.extend(openai_messages_from_api_messages_with_tool_policy(
            &task.messages,
            long_running_shell_controls_advertised,
        ));
    }

    messages.extend(openai_messages_from_inputs(
        &params.input,
        context_char_budget,
    ));
    messages
}

fn openai_messages_from_api_messages_with_tool_policy(
    messages: &[api::Message],
    long_running_shell_controls_advertised: bool,
) -> Vec<ChatMessage> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let Some(api::message::Message::ToolCall(_)) = messages[index].message.as_ref() else {
            output.extend(openai_messages_from_api_message_with_tool_policy(
                &messages[index],
                long_running_shell_controls_advertised,
            ));
            index += 1;
            continue;
        };

        let request_id = &messages[index].request_id;
        if request_id.is_empty() {
            output.extend(openai_messages_from_api_message_with_tool_policy(
                &messages[index],
                long_running_shell_controls_advertised,
            ));
            index += 1;
            continue;
        }

        let mut tool_calls = Vec::new();
        while index < messages.len() {
            let Some(api::message::Message::ToolCall(tool_call)) = messages[index].message.as_ref()
            else {
                break;
            };
            if messages[index].request_id != *request_id {
                break;
            }
            if let Some(openai_tool_call) = openai_tool_call_from_api_tool_call(tool_call) {
                tool_calls.push(openai_tool_call);
            }
            index += 1;
        }
        if !tool_calls.is_empty() {
            output.push(ChatMessage::assistant_tool_calls(tool_calls));
        }
    }
    output
}

fn openai_messages_from_api_message_with_tool_policy(
    message: &api::Message,
    long_running_shell_controls_advertised: bool,
) -> Vec<ChatMessage> {
    match message.message.as_ref() {
        Some(api::message::Message::UserQuery(query)) => {
            vec![ChatMessage::user(with_context(
                query.query.clone(),
                query
                    .context
                    .as_ref()
                    .map(|_| "Context was supplied by Warp."),
            ))]
        }
        Some(api::message::Message::SystemQuery(query)) => {
            let content = match &query.r#type {
                Some(api::message::system_query::Type::AutoCodeDiff(query)) => query.query.clone(),
                Some(api::message::system_query::Type::CreateNewProject(query)) => {
                    query.query.clone()
                }
                Some(api::message::system_query::Type::CloneRepository(query)) => {
                    format!("Clone {}", query.url)
                }
                Some(api::message::system_query::Type::SummarizeConversation(query)) => {
                    query.prompt.clone()
                }
                _ => String::new(),
            };
            (!content.is_empty())
                .then(|| ChatMessage::user(content))
                .into_iter()
                .collect()
        }
        Some(api::message::Message::AgentOutput(output)) => (!output.text.is_empty())
            .then(|| ChatMessage::assistant(output.text.clone()))
            .into_iter()
            .collect(),
        Some(api::message::Message::ToolCall(tool_call)) => {
            let Some(openai_tool_call) = openai_tool_call_from_api_tool_call(tool_call) else {
                return vec![];
            };
            vec![ChatMessage::assistant_tool_call(openai_tool_call)]
        }
        Some(api::message::Message::ToolCallResult(result)) => {
            vec![ChatMessage::tool(
                result.tool_call_id.clone(),
                tool_call_result_to_text_with_tool_policy(
                    result,
                    long_running_shell_controls_advertised,
                ),
            )]
        }
        Some(api::message::Message::AgentReasoning(reasoning)) => (!reasoning.reasoning.is_empty())
            .then(|| ChatMessage::assistant(format!("Reasoning: {}", reasoning.reasoning)))
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

fn openai_messages_from_inputs(
    input: &[AIAgentInput],
    context_char_budget: usize,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    for item in input {
        match item {
            AIAgentInput::UserQuery { query, context, .. } => {
                messages.push(ChatMessage::user(with_context(
                    query.clone(),
                    context_text(context, context_char_budget).as_deref(),
                )));
            }
            AIAgentInput::AutoCodeDiffQuery { query, context }
            | AIAgentInput::CreateNewProject { query, context } => {
                messages.push(ChatMessage::user(with_context(
                    query.clone(),
                    context_text(context, context_char_budget).as_deref(),
                )));
            }
            AIAgentInput::CloneRepository {
                clone_repo_url,
                context,
            } => {
                messages.push(ChatMessage::user(with_context(
                    clone_repo_url.clone().into_url(),
                    context_text(context, context_char_budget).as_deref(),
                )));
            }
            AIAgentInput::SummarizeConversation { prompt, .. } => {
                messages.push(ChatMessage::user(
                    prompt
                        .clone()
                        .unwrap_or_else(|| "Summarize this conversation.".to_string()),
                ));
            }
            AIAgentInput::InvokeSkill {
                skill,
                user_query,
                context,
            } => {
                let query = user_query
                    .as_ref()
                    .map(|query| query.query.clone())
                    .filter(|query| !query.is_empty())
                    .unwrap_or_else(|| format!("Use the {} skill.", skill.name));
                messages.push(ChatMessage::user(with_context(
                    format!(
                        "Invoke Warp skill `{}`.\n\nSkill instructions:\n{}\n\nUser request:\n{}",
                        skill.name, skill.content, query
                    ),
                    context_text(context, context_char_budget).as_deref(),
                )));
            }
            AIAgentInput::ActionResult { result, .. } => {
                messages.push(ChatMessage::tool(
                    result.id.clone().into(),
                    format!("{}", MarkdownActionResult(&result.result)),
                ));
            }
            AIAgentInput::MessagesReceivedFromAgents { messages: received } => {
                let content = received
                    .iter()
                    .map(|message| {
                        format!(
                            "From {} to {:?}: {}\n{}",
                            message.sender_agent_id,
                            message.addresses,
                            message.subject,
                            message.message_body
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !content.is_empty() {
                    messages.push(ChatMessage::user(format!(
                        "Messages received from other agents:\n{content}"
                    )));
                }
            }
            AIAgentInput::EventsFromAgents { events } => {
                if !events.is_empty() {
                    messages.push(ChatMessage::user(format!(
                        "Agent events received:\n{}",
                        events
                            .iter()
                            .map(|event| format!("{event:?}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )));
                }
            }
            AIAgentInput::PassiveSuggestionResult { suggestion, .. } => {
                messages.push(ChatMessage::user(format!(
                    "Passive suggestion result: {suggestion:?}"
                )));
            }
            AIAgentInput::OrchestrationConfigUpdate {
                plan_id,
                config,
                status,
            } => {
                messages.push(ChatMessage::user(format!(
                    "Orchestration config update for plan {plan_id} ({status:?}): {config:?}"
                )));
            }
            AIAgentInput::ResumeConversation { .. }
            | AIAgentInput::InitProjectRules { .. }
            | AIAgentInput::CreateEnvironment { .. }
            | AIAgentInput::TriggerPassiveSuggestion { .. }
            | AIAgentInput::CodeReview { .. }
            | AIAgentInput::StartFromAmbientRunPrompt { .. } => {
                if let Some(query) = item.user_query() {
                    messages.push(ChatMessage::user(query));
                }
            }
        }
    }
    messages
}

fn with_context(mut query: String, context: Option<&str>) -> String {
    let Some(context) = context.filter(|context| !context.trim().is_empty()) else {
        return query;
    };
    query.push_str("\n\nWarp context:\n");
    query.push_str(context);
    query
}

fn context_text(context: &[AIAgentContext], context_char_budget: usize) -> Option<String> {
    let mut parts = Vec::new();
    for item in context {
        match item {
            AIAgentContext::Directory {
                pwd,
                home_dir,
                are_file_symbols_indexed,
            } => parts.push(format!(
                "Directory: pwd={}, home={}, indexed_symbols={}",
                pwd.as_deref().unwrap_or("unknown"),
                home_dir.as_deref().unwrap_or("unknown"),
                are_file_symbols_indexed
            )),
            AIAgentContext::SelectedText(text) => {
                parts.push(format!(
                    "Selected text:\n{}",
                    truncate_context_to(text, context_char_budget)
                ));
            }
            AIAgentContext::ExecutionEnvironment(env) => {
                parts.push(format!("Execution environment: {env:?}"));
            }
            AIAgentContext::CurrentTime { current_time } => {
                parts.push(format!("Current time: {}", current_time.to_rfc3339()));
            }
            AIAgentContext::Image(image) => {
                parts.push(format!(
                    "Attached image: {} ({})",
                    image.file_name, image.mime_type
                ));
            }
            AIAgentContext::Codebase { path, name } => {
                parts.push(format!("Codebase `{name}` at {path}"));
            }
            AIAgentContext::ProjectRules {
                root_path,
                active_rules,
                additional_rule_paths,
            } => {
                let mut text = format!("Project rules for {root_path}:");
                for rule in active_rules {
                    text.push_str(&format!(
                        "\n\n{}:\n{}",
                        rule.file_name,
                        file_context_content(rule, context_char_budget)
                    ));
                }
                if !additional_rule_paths.is_empty() {
                    text.push_str(&format!(
                        "\nAdditional rule paths: {}",
                        additional_rule_paths.join(", ")
                    ));
                }
                parts.push(text);
            }
            AIAgentContext::File(file) => {
                parts.push(format!(
                    "File {}:\n{}",
                    file.file_name,
                    file_context_content(file, context_char_budget)
                ));
            }
            AIAgentContext::Git { head, branch } => {
                parts.push(format!(
                    "Git: head={}, branch={}",
                    head,
                    branch.as_deref().unwrap_or("unknown")
                ));
            }
            AIAgentContext::Skills { skills } => {
                parts.push(format!("Available skills: {skills:?}"));
            }
            AIAgentContext::Block(block) => {
                parts.push(format!(
                    "Terminal block:\ncommand: {}\nexit_code: {}\noutput:\n{}",
                    block.command,
                    block.exit_code.value(),
                    truncate_context_to(&block.output, context_char_budget)
                ));
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(truncate_context_to(
            &parts.join("\n\n"),
            context_char_budget,
        ))
    }
}

fn file_context_content(
    file: &crate::ai::agent::FileContext,
    context_char_budget: usize,
) -> String {
    match &file.content {
        AnyFileContent::StringContent(content) => truncate_context_to(content, context_char_budget),
        AnyFileContent::BinaryContent(_) => "[binary file content omitted]".to_string(),
    }
}

fn truncate_context_to(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .char_indices()
        .take_while(|(idx, _)| *idx < max_chars)
        .map(|(_, ch)| ch)
        .collect::<String>();
    truncated.push_str("\n[truncated]");
    truncated
}

fn system_prompt(
    params: &super::RequestParams,
    tools: &[OpenAITool],
    context_char_budget: usize,
) -> String {
    let mut prompt = String::from(
        "You are Warp Agent running inside the Warp terminal app. Warp is a real local harness, not a plain chat. \
When local tool definitions are enabled, use the provided OpenAI tool-calling interface for shell access, file access, code search, MCP tools, or Warp skills. \
If no tools are listed, do not invent tool calls or claim that an unavailable local tool was run. \
Do not tell the user that you lack tools if tools are listed. The Warp client executes tool calls and sends their results back to you. \
After every tool result, inspect the result and continue with another tool call if the user's request is not complete. \
Only stop with a final answer when the requested work is complete, or when you are blocked and can explain the blocker clearly. \
Do not end the turn merely because one tool call completed. \
For shell tools, set is_read_only=true for inspection-only commands, set is_risky=false for normal inspection/build/test commands, \
and set is_risky=true for destructive, credential-changing, network-sensitive, or externally mutating commands. \
Avoid wrapping commands in sh, bash, zsh, fish, eval, exec, curl, wget, ssh, scp, rsync, or rm unless the task specifically requires it.",
    );

    if let Some(cwd) = params.session_context.current_working_directory() {
        prompt.push_str(&format!("\nCurrent working directory: {cwd}"));
    }

    if tools.is_empty() {
        prompt.push_str("\nNo local tools are currently enabled for this request.");
    } else {
        let tool_names = tools
            .iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>()
            .join(", ");
        prompt.push_str(&format!("\nEnabled Warp tools: {tool_names}."));
    }

    if let Some(mcp_context) = &params.mcp_context {
        let mcp_summary = mcp_context_summary(mcp_context, context_char_budget);
        if !mcp_summary.is_empty() {
            prompt.push_str("\n\nMCP context:\n");
            prompt.push_str(&mcp_summary);
        }
    }

    prompt
}

fn mcp_context_summary(
    context: &crate::ai::agent::MCPContext,
    context_char_budget: usize,
) -> String {
    #[allow(deprecated)]
    let mut lines = context
        .tools
        .iter()
        .map(|tool| {
            format!(
                "- tool `{}`: {}",
                tool.name,
                tool.description.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();

    #[allow(deprecated)]
    lines.extend(
        context
            .resources
            .iter()
            .map(|resource| format!("- resource `{}`", resource.uri)),
    );

    for server in &context.servers {
        lines.push(format!(
            "Server `{}` id={} {}",
            server.name, server.id, server.description
        ));
        for tool in &server.tools {
            lines.push(format!(
                "- server_id={} tool `{}`: {}",
                server.id,
                tool.name,
                tool.description.as_deref().unwrap_or_default()
            ));
        }
        for resource in &server.resources {
            lines.push(format!(
                "- server_id={} resource `{}`",
                server.id, resource.uri
            ));
        }
    }

    truncate_context_to(&lines.join("\n"), context_char_budget)
}

fn api_messages_from_inputs(
    task_id: &str,
    request_id: &str,
    input: &[AIAgentInput],
) -> Vec<api::Message> {
    let mut messages = Vec::new();
    for item in input {
        match item {
            AIAgentInput::UserQuery {
                query,
                referenced_attachments,
                user_query_mode,
                intended_agent,
                ..
            } => {
                messages.push(api::Message {
                    id: Uuid::new_v4().to_string(),
                    task_id: task_id.to_string(),
                    request_id: request_id.to_string(),
                    timestamp: None,
                    server_message_data: String::new(),
                    citations: vec![],
                    message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                        query: query.clone(),
                        context: None,
                        referenced_attachments: referenced_attachments
                            .iter()
                            .map(|(key, attachment)| {
                                (
                                    key.clone(),
                                    api_attachment_from_agent_attachment(attachment.clone()),
                                )
                            })
                            .collect(),
                        mode: Some(api_user_query_mode(*user_query_mode)),
                        intended_agent: intended_agent
                            .map(|agent| agent.into())
                            .unwrap_or_default(),
                    })),
                });
            }
            AIAgentInput::ActionResult { result, .. } => {
                if let Some(tool_result) = api_tool_call_result_from_action_result(result) {
                    messages.push(api::Message {
                        id: Uuid::new_v4().to_string(),
                        task_id: task_id.to_string(),
                        request_id: request_id.to_string(),
                        timestamp: None,
                        server_message_data: String::new(),
                        citations: vec![],
                        message: Some(api::message::Message::ToolCallResult(tool_result)),
                    });
                }
            }
            _ => {}
        }
    }
    messages
}

fn api_tool_call_result_from_action_result(
    result: &AIAgentActionResult,
) -> Option<api::message::ToolCallResult> {
    let result = request_tool_call_result_from_action_result(result)?;

    Some(api::message::ToolCallResult {
        tool_call_id: result.tool_call_id,
        context: None,
        result: result
            .result
            .map(request_tool_result_to_message_tool_result),
    })
}

fn api_user_query_mode(value: UserQueryMode) -> api::UserQueryMode {
    match value {
        UserQueryMode::Normal => api::UserQueryMode { r#type: None },
        UserQueryMode::Plan => api::UserQueryMode {
            r#type: Some(api::user_query_mode::Type::Plan(())),
        },
        UserQueryMode::Orchestrate => api::UserQueryMode {
            r#type: Some(api::user_query_mode::Type::Orchestrate(())),
        },
    }
}

fn api_attachment_from_agent_attachment(attachment: AIAgentAttachment) -> api::Attachment {
    match attachment {
        AIAgentAttachment::PlainText(text) => api::Attachment {
            value: Some(api::attachment::Value::PlainText(text)),
        },
        AIAgentAttachment::Block(block) => api::Attachment {
            value: Some(api::attachment::Value::ExecutedShellCommand(
                api_executed_shell_command_from_block(block),
            )),
        },
        AIAgentAttachment::DriveObject { uid, payload } => api::Attachment {
            value: Some(api::attachment::Value::DriveObject(api::DriveObject {
                uid,
                object_payload: payload.map(|p| match p {
                    DriveObjectPayload::Workflow {
                        name,
                        description,
                        command,
                    } => api::drive_object::ObjectPayload::Workflow(api::Workflow {
                        name,
                        description,
                        command,
                    }),
                    DriveObjectPayload::Notebook { title, content } => {
                        api::drive_object::ObjectPayload::Notebook(api::Notebook { title, content })
                    }
                    DriveObjectPayload::GenericStringObject {
                        payload,
                        object_type,
                    } => api::drive_object::ObjectPayload::GenericStringObject(
                        api::GenericStringObject {
                            payload,
                            object_type,
                        },
                    ),
                }),
            })),
        },
        #[allow(deprecated)]
        AIAgentAttachment::DiffHunk {
            file_path,
            line_range,
            diff_content,
            lines_added,
            lines_removed,
            current,
            base,
        } => api::Attachment {
            value: Some(api::attachment::Value::DiffHunk(api::DiffHunk {
                file_path,
                line_range: Some(api::FileContentLineRange {
                    start: line_range.start.as_usize() as u32,
                    end: line_range.end.as_usize() as u32,
                }),
                diff_content,
                lines_added,
                lines_removed,
                current: current.map(Into::into),
                base: Some(base.into()),
            })),
        },
        AIAgentAttachment::DocumentContent {
            document_id,
            content,
            line_range,
            ..
        } => api::Attachment {
            value: Some(api::attachment::Value::DocumentContent(
                api::DocumentContent {
                    document_id,
                    content,
                    line_range: line_range.map(|range| api::FileContentLineRange {
                        start: range.start.as_usize() as u32,
                        end: range.end.as_usize() as u32,
                    }),
                },
            )),
        },
        AIAgentAttachment::DiffSet {
            file_diffs,
            current,
            base,
        } => api::Attachment {
            value: Some(api::attachment::Value::DiffSet(api::DiffSet {
                hunks: file_diffs
                    .into_iter()
                    .flat_map(|(file_path, hunks)| {
                        hunks
                            .into_iter()
                            .map(move |hunk| hunk.convert_to_api(file_path.clone()))
                    })
                    .collect(),
                curr_ref: current.map(Into::into),
                base_ref: Some(base.into()),
            })),
        },
        AIAgentAttachment::FilePathReference { file_path, .. } => api::Attachment {
            value: Some(api::attachment::Value::FilePathReference(
                api::FilePathReference { file_path },
            )),
        },
    }
}

fn api_executed_shell_command_from_block(block: BlockContext) -> api::ExecutedShellCommand {
    api::ExecutedShellCommand {
        command: block.command,
        output: block.output,
        exit_code: block.exit_code.value(),
        command_id: block.id.into(),
        is_auto_attached: block.is_auto_attached,
        started_ts: block.started_ts.map(local_datetime_to_timestamp),
        finished_ts: block.finished_ts.map(local_datetime_to_timestamp),
    }
}

fn local_datetime_to_timestamp(timestamp: DateTime<Local>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: timestamp.timestamp(),
        nanos: timestamp.timestamp_subsec_nanos() as i32,
    }
}

fn request_tool_call_result_from_action_result(
    action_result: &AIAgentActionResult,
) -> Option<api::request::input::ToolCallResult> {
    let result = match action_result.result.clone() {
        AIAgentActionResultType::RequestCommandOutput(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::WriteToLongRunningShellCommand(result) => {
            Some(result.try_into().ok()?)
        }
        AIAgentActionResultType::ReadFiles(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::SearchCodebase(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::RequestFileEdits(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::Grep(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::FileGlob(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::FileGlobV2(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::ReadMCPResource(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::CallMCPTool(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::ReadSkill(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::SuggestNewConversation(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::SuggestPrompt(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::OpenCodeReview => Some(
            api::request::input::tool_call_result::Result::OpenCodeReview(
                api::OpenCodeReviewResult {},
            ),
        ),
        AIAgentActionResultType::InsertReviewComments(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::InitProject => Some(
            api::request::input::tool_call_result::Result::InitProject(api::InitProjectResult {}),
        ),
        AIAgentActionResultType::ReadDocuments(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::EditDocuments(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::CreateDocuments(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::ReadShellCommandOutput(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::UseComputer(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::RequestComputerUse(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::FetchConversation(result) => Some(result.try_into().ok()?),
        AIAgentActionResultType::StartAgent(result) => Some(result.into()),
        AIAgentActionResultType::SendMessageToAgent(result) => Some(result.into()),
        AIAgentActionResultType::TransferShellCommandControlToUser(result) => {
            Some(result.try_into().ok()?)
        }
        AIAgentActionResultType::AskUserQuestion(result) => Some(result.into()),
        AIAgentActionResultType::RunAgents(result) => Some(result.try_into().ok()?),
    };

    Some(api::request::input::ToolCallResult {
        tool_call_id: action_result.id.to_string(),
        result,
    })
}

fn request_tool_result_to_message_tool_result(
    result: api::request::input::tool_call_result::Result,
) -> api::message::tool_call_result::Result {
    use api::message::tool_call_result::Result as MessageResult;
    use api::request::input::tool_call_result::Result as RequestResult;

    match result {
        RequestResult::RunShellCommand(result) => MessageResult::RunShellCommand(result),
        RequestResult::ReadFiles(result) => MessageResult::ReadFiles(result),
        RequestResult::SearchCodebase(result) => MessageResult::SearchCodebase(result),
        RequestResult::ApplyFileDiffs(result) => MessageResult::ApplyFileDiffs(result),
        RequestResult::SuggestPlan(result) => MessageResult::SuggestPlan(result),
        RequestResult::SuggestCreatePlan(result) => MessageResult::SuggestCreatePlan(result),
        RequestResult::Grep(result) => MessageResult::Grep(result),
        #[allow(deprecated)]
        RequestResult::FileGlob(result) => MessageResult::FileGlob(result),
        RequestResult::ReadMcpResource(result) => MessageResult::ReadMcpResource(result),
        RequestResult::CallMcpTool(result) => MessageResult::CallMcpTool(result),
        RequestResult::WriteToLongRunningShellCommand(result) => {
            MessageResult::WriteToLongRunningShellCommand(result)
        }
        RequestResult::SuggestNewConversation(result) => {
            MessageResult::SuggestNewConversation(result)
        }
        RequestResult::FileGlobV2(result) => MessageResult::FileGlobV2(result),
        RequestResult::SuggestPrompt(result) => MessageResult::SuggestPrompt(result),
        RequestResult::OpenCodeReview(result) => MessageResult::OpenCodeReview(result),
        RequestResult::InitProject(result) => MessageResult::InitProject(result),
        RequestResult::ReadDocuments(result) => MessageResult::ReadDocuments(result),
        RequestResult::EditDocuments(result) => MessageResult::EditDocuments(result),
        RequestResult::CreateDocuments(result) => MessageResult::CreateDocuments(result),
        RequestResult::ReadShellCommandOutput(result) => {
            MessageResult::ReadShellCommandOutput(result)
        }
        RequestResult::UseComputer(result) => MessageResult::UseComputer(result),
        RequestResult::InsertReviewComments(result) => MessageResult::InsertReviewComments(result),
        RequestResult::RequestComputerUse(result) => {
            MessageResult::RequestComputerUseResult(result)
        }
        RequestResult::ReadSkill(result) => MessageResult::ReadSkill(result),
        RequestResult::FetchConversation(result) => MessageResult::FetchConversation(result),
        RequestResult::StartAgent(result) => MessageResult::StartAgent(result),
        RequestResult::SendMessageToAgent(result) => MessageResult::SendMessageToAgent(result),
        RequestResult::TransferShellCommandControlToUser(result) => {
            MessageResult::TransferShellCommandControlToUser(result)
        }
        RequestResult::AskUserQuestion(result) => MessageResult::AskUserQuestion(result),
        RequestResult::StartAgentV2(result) => MessageResult::StartAgentV2(result),
        RequestResult::UploadFileArtifact(result) => MessageResult::UploadFileArtifact(result),
        RequestResult::RunAgentsResult(result) => MessageResult::RunAgentsResult(result),
    }
}

fn openai_tool_call_from_api_tool_call(
    tool_call: &api::message::ToolCall,
) -> Option<OpenAIToolCall> {
    let tool = tool_call.tool.as_ref()?;
    let (name, args) = match tool {
        api::message::tool_call::Tool::RunShellCommand(call) => (
            "run_shell_command",
            json!({
                "command": call.command,
                "is_read_only": call.is_read_only,
                "uses_pager": call.uses_pager,
                "is_risky": call.is_risky,
                "wait_until_completion": call.wait_until_complete_value.as_ref().map(|value| {
                    matches!(
                        value,
                        api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(true)
                    )
                }).unwrap_or(true),
            }),
        ),
        api::message::tool_call::Tool::WriteToLongRunningShellCommand(call) => (
            "write_to_long_running_shell_command",
            json!({
                "command_id": call.command_id,
                "input": if let Ok(input) = String::from_utf8(call.input.clone()) {
                    json!(input)
                } else {
                    json!(call.input)
                },
                "mode": long_running_shell_mode_to_json(call.mode.as_ref()),
            }),
        ),
        api::message::tool_call::Tool::ReadShellCommandOutput(call) => (
            "read_shell_command_output",
            json!({
                "command_id": call.command_id,
                "delay": shell_command_delay_to_json(call.delay.as_ref()),
            }),
        ),
        api::message::tool_call::Tool::TransferShellCommandControlToUser(call) => (
            "transfer_shell_command_control_to_user",
            json!({"reason": call.reason}),
        ),
        api::message::tool_call::Tool::ReadFiles(call) => (
            "read_files",
            json!({
                "files": call.files.iter().map(|file| {
                    json!({
                        "name": file.name,
                        "line_ranges": file.line_ranges.iter().map(|range| {
                            json!({"start": range.start, "end": range.end})
                        }).collect::<Vec<_>>()
                    })
                }).collect::<Vec<_>>()
            }),
        ),
        api::message::tool_call::Tool::SearchCodebase(call) => (
            "search_codebase",
            json!({
                "query": call.query,
                "path_filters": call.path_filters,
                "codebase_path": call.codebase_path,
            }),
        ),
        api::message::tool_call::Tool::Grep(call) => {
            ("grep", json!({"queries": call.queries, "path": call.path}))
        }
        api::message::tool_call::Tool::FileGlobV2(call) => (
            "file_glob",
            json!({
                "patterns": call.patterns,
                "search_dir": call.search_dir,
                "max_matches": call.max_matches,
                "max_depth": call.max_depth,
                "min_depth": call.min_depth,
            }),
        ),
        api::message::tool_call::Tool::ReadMcpResource(call) => (
            "read_mcp_resource",
            json!({"uri": call.uri, "server_id": call.server_id}),
        ),
        api::message::tool_call::Tool::CallMcpTool(call) => (
            "call_mcp_tool",
            json!({
                "name": call.name,
                "server_id": call.server_id,
                "args": call.args.as_ref().map(prost_struct_to_json).unwrap_or_else(|| json!({})),
            }),
        ),
        api::message::tool_call::Tool::ReadSkill(call) => {
            let (skill_path, bundled_skill_id) = match call.skill_reference.as_ref() {
                Some(api::message::tool_call::read_skill::SkillReference::SkillPath(path)) => {
                    (path.clone(), String::new())
                }
                Some(api::message::tool_call::read_skill::SkillReference::BundledSkillId(id)) => {
                    (String::new(), id.clone())
                }
                None => (String::new(), String::new()),
            };
            (
                "read_skill",
                json!({
                    "skill_path": skill_path,
                    "bundled_skill_id": bundled_skill_id,
                    "name": call.name,
                }),
            )
        }
        api::message::tool_call::Tool::ApplyFileDiffs(call) => (
            "apply_file_diffs",
            json!({
                "summary": call.summary,
                "diffs": call.diffs.iter().map(|diff| json!({
                    "file_path": diff.file_path,
                    "search": diff.search,
                    "replace": diff.replace,
                })).collect::<Vec<_>>(),
                "new_files": call.new_files.iter().map(|file| json!({
                    "file_path": file.file_path,
                    "content": file.content,
                })).collect::<Vec<_>>(),
                "deleted_files": call.deleted_files.iter().map(|file| json!({
                    "file_path": file.file_path,
                })).collect::<Vec<_>>(),
            }),
        ),
        api::message::tool_call::Tool::SuggestNewConversation(call) => (
            "suggest_new_conversation",
            json!({"message_id": call.message_id}),
        ),
        api::message::tool_call::Tool::OpenCodeReview(_) => ("open_code_review", json!({})),
        api::message::tool_call::Tool::InitProject(_) => ("init_project", json!({})),
        api::message::tool_call::Tool::ReadDocuments(call) => (
            "read_documents",
            json!({
                "documents": call.documents.iter().map(|document| json!({
                    "document_id": document.document_id,
                    "line_ranges": document.line_ranges.iter().map(|range| {
                        json!({"start": range.start, "end": range.end})
                    }).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }),
        ),
        api::message::tool_call::Tool::EditDocuments(call) => (
            "edit_documents",
            json!({
                "diffs": call.diffs.iter().map(|diff| json!({
                    "document_id": diff.document_id,
                    "search": diff.search,
                    "replace": diff.replace,
                })).collect::<Vec<_>>(),
            }),
        ),
        api::message::tool_call::Tool::CreateDocuments(call) => (
            "create_documents",
            json!({
                "documents": call.new_documents.iter().map(|document| json!({
                    "content": document.content,
                    "title": document.title,
                })).collect::<Vec<_>>(),
            }),
        ),
        api::message::tool_call::Tool::InsertReviewComments(call) => (
            "insert_review_comments",
            json!({
                "repo_path": call.repo_path,
                "comments": call.comments.iter().map(review_comment_to_json).collect::<Vec<_>>(),
                "base_branch": call.base_branch,
            }),
        ),
        api::message::tool_call::Tool::FetchConversation(call) => (
            "fetch_conversation",
            json!({"conversation_id": call.conversation_id}),
        ),
        api::message::tool_call::Tool::AskUserQuestion(call) => (
            "ask_user_question",
            json!({
                "questions": call.questions.iter().map(ask_user_question_to_json).collect::<Vec<_>>(),
            }),
        ),
        api::message::tool_call::Tool::UseComputer(call) => (
            "use_computer",
            json!({
                "actions": call.actions.iter().map(use_computer_action_to_json).collect::<Vec<_>>(),
                "post_actions_screenshot_params": call.post_actions_screenshot_params.as_ref().map(screenshot_params_to_json),
                "action_summary": call.action_summary,
            }),
        ),
        api::message::tool_call::Tool::RequestComputerUse(call) => (
            "request_computer_use",
            json!({
                "task_summary": call.task_summary,
                "screenshot_params": call.screenshot_params.as_ref().map(screenshot_params_to_json),
            }),
        ),
        _ => return None,
    };

    Some(OpenAIToolCall {
        id: tool_call.tool_call_id.clone(),
        kind: "function".to_string(),
        function: OpenAIFunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        },
    })
}

fn long_running_shell_mode_to_json(
    mode: Option<&api::message::tool_call::write_to_long_running_shell_command::Mode>,
) -> Value {
    use api::message::tool_call::write_to_long_running_shell_command::mode::Mode;
    match mode.and_then(|mode| mode.mode.as_ref()) {
        Some(Mode::Line(())) => json!("line"),
        Some(Mode::Block(())) => json!("block"),
        Some(Mode::Raw(())) => json!("raw"),
        None => Value::Null,
    }
}

fn shell_command_delay_to_json(
    delay: Option<&api::message::tool_call::read_shell_command_output::Delay>,
) -> Value {
    use api::message::tool_call::read_shell_command_output::Delay;
    match delay {
        Some(Delay::Duration(duration)) => json!({
            "kind": "duration",
            "seconds": duration.seconds,
            "nanos": duration.nanos,
        }),
        Some(Delay::OnCompletion(())) => json!("on_completion"),
        None => Value::Null,
    }
}

fn review_comment_to_json(
    comment: &api::message::tool_call::insert_review_comments::Comment,
) -> Value {
    json!({
        "comment_id": comment.comment_id,
        "author": comment.author,
        "last_modified_timestamp": comment.last_modified_timestamp,
        "comment_body": comment.comment_body,
        "parent_comment_id": comment.parent_comment_id,
        "location": comment.location.as_ref().map(review_comment_location_to_json),
        "html_url": comment.html_url,
    })
}

fn review_comment_location_to_json(
    location: &api::message::tool_call::insert_review_comments::CommentLocation,
) -> Value {
    json!({
        "file_path": location.file_path,
        "line": location.line.as_ref().map(review_comment_line_to_json),
    })
}

fn review_comment_line_to_json(
    line: &api::message::tool_call::insert_review_comments::CommentLineRange,
) -> Value {
    let side =
        match api::message::tool_call::insert_review_comments::CommentSide::try_from(line.side) {
            Ok(api::message::tool_call::insert_review_comments::CommentSide::Old) => "OLD",
            _ => "NEW",
        };
    json!({
        "diff_hunk": line.diff_hunk,
        "range": line.range.as_ref().map(|range| json!({
            "start": range.start,
            "end": range.end,
        })),
        "side": side,
    })
}

fn ask_user_question_to_json(question: &api::ask_user_question::Question) -> Value {
    let multiple_choice = match question.question_type.as_ref() {
        Some(api::ask_user_question::question::QuestionType::MultipleChoice(multiple_choice)) => {
            json!({
                "options": multiple_choice.options.iter().map(|option| {
                    json!({"label": option.label})
                }).collect::<Vec<_>>(),
                "recommended_option_index": multiple_choice.recommended_option_index,
                "is_multiselect": multiple_choice.is_multiselect,
                "supports_other": multiple_choice.supports_other,
            })
        }
        None => Value::Null,
    };
    json!({
        "question_id": question.question_id,
        "question": question.question,
        "multiple_choice": multiple_choice,
    })
}

fn coordinates_to_json(coordinates: Option<&api::Coordinates>) -> Value {
    coordinates
        .map(|coordinates| json!({"x": coordinates.x, "y": coordinates.y}))
        .unwrap_or(Value::Null)
}

fn screenshot_params_to_json(params: &api::message::tool_call::ScreenshotParams) -> Value {
    let region = params.region.as_ref().and_then(|region| {
        Some(json!({
            "top_left": coordinates_to_json(region.top_left.as_ref()),
            "bottom_right": coordinates_to_json(region.bottom_right.as_ref()),
        }))
        .filter(|_| region.top_left.is_some() && region.bottom_right.is_some())
    });
    json!({
        "max_long_edge_px": params.max_long_edge_px,
        "max_total_px": params.max_total_px,
        "region": region,
    })
}

fn use_computer_action_to_json(action: &api::message::tool_call::use_computer::Action) -> Value {
    use api::message::tool_call::use_computer::action::Type;
    match action.r#type.as_ref() {
        Some(Type::MouseMove(mouse_move)) => json!({
            "type": "mouse_move",
            "to": coordinates_to_json(mouse_move.to.as_ref()),
        }),
        Some(Type::MouseDown(mouse_down)) => json!({
            "type": "mouse_down",
            "button": mouse_button_name(mouse_down.button),
            "at": coordinates_to_json(mouse_down.at.as_ref()),
        }),
        Some(Type::MouseUp(mouse_up)) => json!({
            "type": "mouse_up",
            "button": mouse_button_name(mouse_up.button),
        }),
        Some(Type::MouseWheel(mouse_wheel)) => json!({
            "type": "mouse_wheel",
            "at": coordinates_to_json(mouse_wheel.at.as_ref()),
            "direction": scroll_direction_name(mouse_wheel.direction),
            "distance": mouse_wheel_distance_to_json(mouse_wheel.distance.as_ref()),
        }),
        Some(Type::Wait(wait)) => json!({
            "type": "wait",
            "seconds": wait.duration.as_ref().map(|duration| json!({
                "seconds": duration.seconds,
                "nanos": duration.nanos,
            })),
        }),
        Some(Type::TypeText(type_text)) => json!({
            "type": "type_text",
            "text": type_text.text,
        }),
        Some(Type::KeyDown(key_down)) => json!({
            "type": "key_down",
            "key": computer_key_to_json(key_down.key.as_ref()),
        }),
        Some(Type::KeyUp(key_up)) => json!({
            "type": "key_up",
            "key": computer_key_to_json(key_up.key.as_ref()),
        }),
        None => json!({"type": "unknown"}),
    }
}

fn mouse_button_name(value: i32) -> &'static str {
    match api::message::tool_call::use_computer::action::MouseButton::try_from(value) {
        Ok(api::message::tool_call::use_computer::action::MouseButton::Right) => "right",
        Ok(api::message::tool_call::use_computer::action::MouseButton::Middle) => "middle",
        Ok(api::message::tool_call::use_computer::action::MouseButton::Back) => "back",
        Ok(api::message::tool_call::use_computer::action::MouseButton::Forward) => "forward",
        _ => "left",
    }
}

fn scroll_direction_name(value: i32) -> &'static str {
    match api::message::tool_call::use_computer::action::mouse_wheel::Direction::try_from(value) {
        Ok(api::message::tool_call::use_computer::action::mouse_wheel::Direction::Down) => "down",
        Ok(api::message::tool_call::use_computer::action::mouse_wheel::Direction::Left) => "left",
        Ok(api::message::tool_call::use_computer::action::mouse_wheel::Direction::Right) => "right",
        _ => "up",
    }
}

fn mouse_wheel_distance_to_json(
    distance: Option<&api::message::tool_call::use_computer::action::mouse_wheel::Distance>,
) -> Value {
    use api::message::tool_call::use_computer::action::mouse_wheel::Distance;
    match distance {
        Some(Distance::Pixels(value)) => json!({"pixels": value}),
        Some(Distance::Clicks(value)) => json!({"clicks": value}),
        None => Value::Null,
    }
}

fn computer_key_to_json(key: Option<&api::message::tool_call::use_computer::action::Key>) -> Value {
    use api::message::tool_call::use_computer::action::key::Data;
    match key.and_then(|key| key.data.as_ref()) {
        Some(Data::Keycode(value)) => json!({"keycode": value}),
        Some(Data::Char(value)) => json!({"char": value}),
        None => Value::Null,
    }
}

fn api_tool_call_message(
    task_id: &str,
    request_id: &str,
    tool_call: OpenAIToolCall,
) -> Result<api::Message, AIApiError> {
    api_tool_call_message_inner(task_id, request_id, tool_call, None, false)
}

fn api_tool_call_message_for_supported_tools(
    task_id: &str,
    request_id: &str,
    tool_call: OpenAIToolCall,
    supported_tools: &[api::ToolType],
    long_running_shell_controls_advertised: bool,
) -> Result<api::Message, AIApiError> {
    api_tool_call_message_inner(
        task_id,
        request_id,
        tool_call,
        Some(supported_tools),
        long_running_shell_controls_advertised,
    )
}

fn api_tool_call_message_inner(
    task_id: &str,
    request_id: &str,
    tool_call: OpenAIToolCall,
    supported_tools: Option<&[api::ToolType]>,
    long_running_shell_controls_advertised: bool,
) -> Result<api::Message, AIApiError> {
    log::info!(
        "OpenAI-compatible custom provider requested Warp tool call: {}",
        tool_call.function.name
    );
    let tool = api_tool_from_openai_tool_call_inner(
        &tool_call,
        supported_tools,
        long_running_shell_controls_advertised,
    )?;
    Ok(api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_string(),
        request_id: request_id.to_string(),
        timestamp: None,
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: tool_call.id,
            tool: Some(tool),
        })),
    })
}

fn api_tool_from_openai_tool_call(
    tool_call: &OpenAIToolCall,
) -> Result<api::message::tool_call::Tool, AIApiError> {
    // This helper is also used for persisted history round-trips.  It has no
    // request capability context, so preserve an explicit asynchronous wait;
    // the request-boundary callers use the policy-aware helper below.
    api_tool_from_openai_tool_call_inner(tool_call, None, true)
}

fn api_tool_from_openai_tool_call_with_supported_tools(
    tool_call: &OpenAIToolCall,
    supported_tools: &[api::ToolType],
) -> Result<api::message::tool_call::Tool, AIApiError> {
    api_tool_from_openai_tool_call_inner(
        tool_call,
        Some(supported_tools),
        supports_complete_long_running_shell_family(supported_tools),
    )
}

fn api_tool_from_openai_tool_call_inner(
    tool_call: &OpenAIToolCall,
    supported_tools: Option<&[api::ToolType]>,
    long_running_shell_controls_advertised: bool,
) -> Result<api::message::tool_call::Tool, AIApiError> {
    let args = validate_openai_tool_call_envelope(tool_call)?;

    let tool_name = tool_call.function.name.as_str();
    if let Some(supported_tools) = supported_tools {
        let is_supported = tool_type_for_openai_name(tool_name)
            .is_some_and(|tool_type| tool_type_is_advertised(tool_type, supported_tools));
        if !is_supported {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible provider called local tool `{tool_name}` that is not advertised for this request"
            )));
        }
    }

    match tool_name {
        "run_shell_command" => {
            let command = required_string(&args, "command")?;
            let inferred_flags = infer_shell_command_flags(&command);
            let requested_wait_until_completion =
                optional_bool_strict(&args, "wait_until_completion")?.unwrap_or(true);
            let wait_until_completion = if long_running_shell_controls_advertised {
                requested_wait_until_completion
            } else {
                if !requested_wait_until_completion {
                    log::warn!(
                        "OpenAI-compatible provider requested wait_until_completion=false for run_shell_command; forcing completion wait because the direct provider path does not expose long-running command polling tools"
                    );
                }
                true
            };
            if !wait_until_completion {
                log::warn!(
                    "OpenAI-compatible provider requested a long-running run_shell_command; preserving the asynchronous request because all local long-running shell controls are advertised"
                );
            }
            Ok(api::message::tool_call::Tool::RunShellCommand(
                api::message::tool_call::RunShellCommand {
                    command,
                    is_read_only: optional_bool_strict(&args, "is_read_only")?
                        .unwrap_or(inferred_flags.is_read_only),
                    uses_pager: optional_bool_strict(&args, "uses_pager")?.unwrap_or(false),
                    citations: vec![],
                    is_risky: optional_bool_strict(&args, "is_risky")?
                        .unwrap_or(inferred_flags.is_risky),
                    wait_until_complete_value: Some(
                        api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(
                            wait_until_completion,
                        ),
                    ),
                    risk_category: 0,
                },
            ))
        }
        "read_files" => Ok(api::message::tool_call::Tool::ReadFiles(
            api::message::tool_call::ReadFiles {
                files: array(&args, "files")?
                    .iter()
                    .map(read_file_arg)
                    .collect::<Result<Vec<_>, _>>()?,
            },
        )),
        "search_codebase" => Ok(api::message::tool_call::Tool::SearchCodebase(
            api::message::tool_call::SearchCodebase {
                query: required_string(&args, "query")?,
                path_filters: optional_string_array_strict(&args, "path_filters")?
                    .unwrap_or_default(),
                codebase_path: optional_string_strict(&args, "codebase_path")?.unwrap_or_default(),
            },
        )),
        "grep" => Ok(api::message::tool_call::Tool::Grep(
            api::message::tool_call::Grep {
                queries: string_array_or_single(&args, "queries", "query")?,
                path: optional_string_strict(&args, "path")?.unwrap_or_default(),
            },
        )),
        "file_glob" => Ok(api::message::tool_call::Tool::FileGlobV2(
            api::message::tool_call::FileGlobV2 {
                patterns: string_array_or_single(&args, "patterns", "pattern")?,
                search_dir: optional_string_strict(&args, "search_dir")?
                    .or(optional_string_strict(&args, "path")?)
                    .unwrap_or_default(),
                max_matches: nonnegative_optional_i32(&args, "max_matches")?.unwrap_or_default(),
                max_depth: nonnegative_optional_i32(&args, "max_depth")?.unwrap_or_default(),
                min_depth: nonnegative_optional_i32(&args, "min_depth")?.unwrap_or_default(),
            },
        )),
        "read_mcp_resource" => Ok(api::message::tool_call::Tool::ReadMcpResource(
            api::message::tool_call::ReadMcpResource {
                uri: required_string(&args, "uri")?,
                server_id: optional_string_strict(&args, "server_id")?.unwrap_or_default(),
            },
        )),
        "call_mcp_tool" => {
            let input = optional_value_strict(&args, "args")?
                .cloned()
                .unwrap_or_else(|| json!({}));
            let prost_types::Value {
                kind: Some(prost_types::value::Kind::StructValue(tool_args)),
            } = serde_json_to_prost(input)
                .map_err(|error| AIApiError::Other(anyhow::anyhow!(error)))?
            else {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "call_mcp_tool args must be a JSON object"
                )));
            };
            Ok(api::message::tool_call::Tool::CallMcpTool(
                api::message::tool_call::CallMcpTool {
                    name: required_string_from_alias(&args, &["name", "tool"])?,
                    args: Some(tool_args),
                    server_id: optional_string_strict(&args, "server_id")?.unwrap_or_default(),
                },
            ))
        }
        "read_skill" => {
            let skill_path = optional_string_strict(&args, "skill_path")?.filter(|s| !s.is_empty());
            let bundled_skill_id =
                optional_string_strict(&args, "bundled_skill_id")?.filter(|s| !s.is_empty());
            let skill_reference = match (skill_path, bundled_skill_id) {
                (Some(skill_path), None) => {
                    Some(api::message::tool_call::read_skill::SkillReference::SkillPath(skill_path))
                }
                (None, Some(bundled_skill_id)) => Some(
                    api::message::tool_call::read_skill::SkillReference::BundledSkillId(
                        bundled_skill_id,
                    ),
                ),
                (Some(_), Some(_)) => {
                    return Err(AIApiError::Other(anyhow::anyhow!(
                        "read_skill requires exactly one of skill_path or bundled_skill_id"
                    )));
                }
                (None, None) => {
                    return Err(AIApiError::Other(anyhow::anyhow!(
                        "read_skill requires a non-empty skill_path or bundled_skill_id"
                    )));
                }
            };
            Ok(api::message::tool_call::Tool::ReadSkill(
                api::message::tool_call::ReadSkill {
                    skill_reference,
                    name: optional_string_strict(&args, "name")?.unwrap_or_default(),
                },
            ))
        }
        "apply_file_diffs" => Ok(api::message::tool_call::Tool::ApplyFileDiffs(
            apply_file_diffs_arg(&args)?,
        )),
        "suggest_new_conversation" => Ok(api::message::tool_call::Tool::SuggestNewConversation(
            api::message::tool_call::SuggestNewConversation {
                message_id: required_string(&args, "message_id")?,
            },
        )),
        "open_code_review" => {
            reject_unexpected_arguments(&args, "open_code_review")?;
            Ok(api::message::tool_call::Tool::OpenCodeReview(
                api::message::tool_call::OpenCodeReview {},
            ))
        }
        "init_project" => {
            reject_unexpected_arguments(&args, "init_project")?;
            Ok(api::message::tool_call::Tool::InitProject(
                api::message::tool_call::InitProject {},
            ))
        }
        "read_documents" => Ok(api::message::tool_call::Tool::ReadDocuments(
            read_documents_arg(&args)?,
        )),
        "edit_documents" => Ok(api::message::tool_call::Tool::EditDocuments(
            edit_documents_arg(&args)?,
        )),
        "create_documents" => Ok(api::message::tool_call::Tool::CreateDocuments(
            create_documents_arg(&args)?,
        )),
        "read_shell_command_output" => Ok(api::message::tool_call::Tool::ReadShellCommandOutput(
            read_shell_command_output_arg(&args)?,
        )),
        "write_to_long_running_shell_command" => Ok(
            api::message::tool_call::Tool::WriteToLongRunningShellCommand(
                write_to_long_running_shell_command_arg(&args)?,
            ),
        ),
        "transfer_shell_command_control_to_user" => Ok(
            api::message::tool_call::Tool::TransferShellCommandControlToUser(
                api::message::tool_call::TransferShellCommandControlToUser {
                    reason: required_string(&args, "reason")?,
                },
            ),
        ),
        "insert_review_comments" => Ok(api::message::tool_call::Tool::InsertReviewComments(
            insert_review_comments_arg(&args)?,
        )),
        "fetch_conversation" => Ok(api::message::tool_call::Tool::FetchConversation(
            api::message::tool_call::FetchConversation {
                conversation_id: required_string(&args, "conversation_id")?,
            },
        )),
        "ask_user_question" => Ok(api::message::tool_call::Tool::AskUserQuestion(
            ask_user_question_arg(&args)?,
        )),
        "use_computer" => Ok(api::message::tool_call::Tool::UseComputer(
            use_computer_arg(&args)?,
        )),
        "request_computer_use" => Ok(api::message::tool_call::Tool::RequestComputerUse(
            request_computer_use_arg(&args)?,
        )),
        other => Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible provider called unsupported Warp tool `{other}`"
        ))),
    }
}

fn validate_openai_tool_call_envelope(tool_call: &OpenAIToolCall) -> Result<Value, AIApiError> {
    if tool_call.id.trim().is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible tool call was missing a non-empty id"
        )));
    }
    if tool_call.kind != "function" {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible tool call type must be exactly `function`, got `{}`",
            tool_call.kind
        )));
    }
    if tool_call.function.name.trim().is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible tool call was missing a non-empty function name"
        )));
    }
    if tool_call.function.arguments.trim().is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible tool call arguments must be a non-empty JSON object"
        )));
    }
    let args: Value = serde_json::from_str(&tool_call.function.arguments).with_context(|| {
        format!(
            "failed to decode arguments for OpenAI-compatible tool `{}`",
            tool_call.function.name
        )
    })?;
    if !args.is_object() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "arguments for OpenAI-compatible tool `{}` must be a JSON object",
            tool_call.function.name
        )));
    }
    Ok(args)
}

fn reject_unexpected_arguments(args: &Value, tool_name: &str) -> Result<(), AIApiError> {
    let object = args.as_object().ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!(
            "arguments for OpenAI-compatible tool `{tool_name}` must be a JSON object"
        ))
    })?;
    if let Some(key) = object.keys().next() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible tool `{tool_name}` does not accept argument `{key}`"
        )));
    }
    Ok(())
}

fn read_documents_arg(args: &Value) -> Result<api::message::tool_call::ReadDocuments, AIApiError> {
    let documents = array(args, "documents")?;
    if documents.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "read_documents requires at least one document"
        )));
    }
    Ok(api::message::tool_call::ReadDocuments {
        documents: documents
            .iter()
            .map(|document| {
                let document_id = valid_document_id(required_string(document, "document_id")?)?;
                Ok(api::message::tool_call::read_documents::Document {
                    document_id,
                    line_ranges: line_ranges_arg(document)?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
    })
}

fn edit_documents_arg(args: &Value) -> Result<api::message::tool_call::EditDocuments, AIApiError> {
    let diffs = array(args, "diffs")?;
    if diffs.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "edit_documents requires at least one diff"
        )));
    }
    Ok(api::message::tool_call::EditDocuments {
        diffs: diffs
            .iter()
            .map(|diff| {
                Ok(api::message::tool_call::edit_documents::DocumentDiff {
                    document_id: valid_document_id(required_string(diff, "document_id")?)?,
                    search: required_string(diff, "search")?,
                    replace: required_string(diff, "replace")?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
    })
}

fn create_documents_arg(
    args: &Value,
) -> Result<api::message::tool_call::CreateDocuments, AIApiError> {
    let documents = optional_array_strict(args, "documents")?
        .or(optional_array_strict(args, "new_documents")?)
        .ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!(
                "missing array argument `documents` for create_documents"
            ))
        })?;
    if documents.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "create_documents requires at least one document"
        )));
    }
    Ok(api::message::tool_call::CreateDocuments {
        new_documents: documents
            .iter()
            .map(|document| {
                Ok(api::message::tool_call::create_documents::NewDocument {
                    content: required_string(document, "content")?,
                    title: optional_string_strict(document, "title")?.unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
    })
}

fn line_ranges_arg(value: &Value) -> Result<Vec<api::FileContentLineRange>, AIApiError> {
    let Some(line_ranges) = optional_value_strict(value, "line_ranges")? else {
        return Ok(Vec::new());
    };
    let line_ranges = line_ranges.as_array().ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!("argument `line_ranges` must be an array"))
    })?;
    line_ranges
        .iter()
        .map(|range| {
            let start = required_u32(range, "start")?;
            let end = required_u32(range, "end")?;
            if start > end {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "line range `start` must not exceed `end`"
                )));
            }
            Ok(api::FileContentLineRange { start, end })
        })
        .collect()
}

fn valid_document_id(value: String) -> Result<String, AIApiError> {
    uuid::Uuid::try_parse(&value).map_err(|error| {
        AIApiError::Other(anyhow::anyhow!(
            "invalid document_id `{value}`; expected a UUID: {error}"
        ))
    })?;
    Ok(value)
}

fn read_shell_command_output_arg(
    args: &Value,
) -> Result<api::message::tool_call::ReadShellCommandOutput, AIApiError> {
    let command_id = required_string(args, "command_id")?;
    let delay = if let Some(delay) = optional_value_strict(args, "delay")? {
        parse_shell_command_delay(delay)?
    } else if let Some(seconds) = optional_i64_strict(args, "delay_seconds")? {
        if seconds < 0 {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "delay_seconds must not be negative"
            )));
        }
        Some(
            api::message::tool_call::read_shell_command_output::Delay::Duration(
                prost_types::Duration { seconds, nanos: 0 },
            ),
        )
    } else {
        None
    };
    Ok(api::message::tool_call::ReadShellCommandOutput { command_id, delay })
}

fn parse_shell_command_delay(
    value: &Value,
) -> Result<Option<api::message::tool_call::read_shell_command_output::Delay>, AIApiError> {
    if let Some(kind) = value.as_str() {
        return match kind {
            "on_completion" | "on-completion" => Ok(Some(
                api::message::tool_call::read_shell_command_output::Delay::OnCompletion(()),
            )),
            _ => Err(AIApiError::Other(anyhow::anyhow!(
                "unsupported shell command delay `{kind}`"
            ))),
        };
    }
    value.as_object().ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!(
            "argument `delay` must be `on_completion` or an object with kind and seconds"
        ))
    })?;
    let kind = optional_string_strict(value, "kind")?
        .or(optional_string_strict(value, "type")?)
        .unwrap_or_else(|| "duration".to_string());
    match kind.as_str() {
        "duration" | "seconds" => {
            let seconds = optional_i64_strict(value, "seconds")?
                .or(optional_i64_strict(value, "duration_seconds")?)
                .ok_or_else(|| {
                    AIApiError::Other(anyhow::anyhow!("duration delay requires integer `seconds`"))
                })?;
            let nanos = optional_i32_strict(value, "nanos")?.unwrap_or_default();
            if seconds < 0 {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "duration delay seconds must not be negative"
                )));
            }
            if !(0..1_000_000_000).contains(&nanos) {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "duration delay nanos must be between 0 and 999999999"
                )));
            }
            Ok(Some(
                api::message::tool_call::read_shell_command_output::Delay::Duration(
                    prost_types::Duration { seconds, nanos },
                ),
            ))
        }
        "on_completion" | "on-completion" => Ok(Some(
            api::message::tool_call::read_shell_command_output::Delay::OnCompletion(()),
        )),
        _ => Err(AIApiError::Other(anyhow::anyhow!(
            "unsupported shell command delay `{kind}`"
        ))),
    }
}

fn write_to_long_running_shell_command_arg(
    args: &Value,
) -> Result<api::message::tool_call::WriteToLongRunningShellCommand, AIApiError> {
    let command_id = required_string(args, "command_id")?;
    let input = if let Some(input) = args.get("input") {
        match input {
            Value::String(input) => input.as_bytes().to_vec(),
            Value::Array(bytes) => bytes
                .iter()
                .map(|byte| {
                    byte.as_u64()
                        .and_then(|byte| u8::try_from(byte).ok())
                        .ok_or_else(|| {
                            AIApiError::Other(anyhow::anyhow!(
                                "long-running shell `input` byte values must be integers from 0 to 255"
                            ))
                        })
                })
                .collect::<Result<Vec<_>, AIApiError>>()?,
            _ => {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "long-running shell `input` must be a string or byte array"
                )));
            }
        }
    } else {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "missing string argument `input`"
        )));
    };
    let mode = optional_string_strict(args, "mode")?
        .map(|mode| {
            match mode.to_ascii_lowercase().as_str() {
            "raw" => Ok(api::message::tool_call::write_to_long_running_shell_command::Mode {
                mode: Some(
                    api::message::tool_call::write_to_long_running_shell_command::mode::Mode::Raw(
                        (),
                    ),
                ),
            }),
            "line" => Ok(api::message::tool_call::write_to_long_running_shell_command::Mode {
                mode: Some(
                    api::message::tool_call::write_to_long_running_shell_command::mode::Mode::Line(
                        (),
                    ),
                ),
            }),
            "block" => Ok(api::message::tool_call::write_to_long_running_shell_command::Mode {
                mode: Some(
                    api::message::tool_call::write_to_long_running_shell_command::mode::Mode::Block(
                        (),
                    ),
                ),
            }),
            other => Err(AIApiError::Other(anyhow::anyhow!(
                "unsupported long-running shell input mode `{other}`"
            ))),
        }
        })
        .transpose()?;
    Ok(api::message::tool_call::WriteToLongRunningShellCommand {
        input,
        mode,
        command_id,
    })
}

fn insert_review_comments_arg(
    args: &Value,
) -> Result<api::message::tool_call::InsertReviewComments, AIApiError> {
    let comments = array(args, "comments")?;
    if comments.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "insert_review_comments requires at least one comment"
        )));
    }
    Ok(api::message::tool_call::InsertReviewComments {
        repo_path: required_string(args, "repo_path")?,
        comments: comments
            .iter()
            .map(insert_review_comment_arg)
            .collect::<Result<Vec<_>, AIApiError>>()?,
        base_branch: optional_string_strict(args, "base_branch")?.unwrap_or_default(),
    })
}

fn insert_review_comment_arg(
    value: &Value,
) -> Result<api::message::tool_call::insert_review_comments::Comment, AIApiError> {
    let location = value
        .get("location")
        .filter(|location| !location.is_null())
        .map(insert_review_comment_location_arg)
        .transpose()?;
    Ok(api::message::tool_call::insert_review_comments::Comment {
        comment_id: required_string(value, "comment_id")?,
        author: optional_string_strict(value, "author")?.unwrap_or_default(),
        last_modified_timestamp: optional_string_strict(value, "last_modified_timestamp")?
            .unwrap_or_default(),
        comment_body: required_string(value, "comment_body")?,
        parent_comment_id: optional_string_strict(value, "parent_comment_id")?.unwrap_or_default(),
        location,
        html_url: optional_string_strict(value, "html_url")?.unwrap_or_default(),
    })
}

fn insert_review_comment_location_arg(
    value: &Value,
) -> Result<api::message::tool_call::insert_review_comments::CommentLocation, AIApiError> {
    Ok(
        api::message::tool_call::insert_review_comments::CommentLocation {
            file_path: required_string(value, "file_path")?,
            line: value
                .get("line")
                .filter(|line| !line.is_null())
                .map(insert_review_comment_line_arg)
                .transpose()?,
        },
    )
}

fn insert_review_comment_line_arg(
    value: &Value,
) -> Result<api::message::tool_call::insert_review_comments::CommentLineRange, AIApiError> {
    let side = optional_string_strict(value, "side")?
        .unwrap_or_else(|| "NEW".to_string())
        .to_ascii_uppercase();
    let side = match side.as_str() {
        "NEW" | "RIGHT" => api::message::tool_call::insert_review_comments::CommentSide::New as i32,
        "OLD" | "LEFT" => api::message::tool_call::insert_review_comments::CommentSide::Old as i32,
        _ => {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "unsupported review comment side; expected NEW or OLD"
            )));
        }
    };
    Ok(
        api::message::tool_call::insert_review_comments::CommentLineRange {
            diff_hunk: optional_string_strict(value, "diff_hunk")?.unwrap_or_default(),
            range: value
                .get("range")
                .filter(|range| !range.is_null())
                .map(|range| {
                    let start = required_u32(range, "start")?;
                    let end = required_u32(range, "end")?;
                    if start > end {
                        return Err(AIApiError::Other(anyhow::anyhow!(
                            "review comment line range `start` must not exceed `end`"
                        )));
                    }
                    Ok(api::FileContentLineRange { start, end })
                })
                .transpose()?,
            side,
        },
    )
}

fn ask_user_question_arg(args: &Value) -> Result<api::AskUserQuestion, AIApiError> {
    let questions = array(args, "questions")?;
    if questions.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "ask_user_question requires at least one question"
        )));
    }
    Ok(api::AskUserQuestion {
        questions: questions
            .iter()
            .map(|question| {
                let multiple_choice = match optional_value_strict(question, "multiple_choice")? {
                    None => {
                        return Ok(api::ask_user_question::Question {
                            question_id: required_string(question, "question_id")?,
                            question: required_string(question, "question")?,
                            question_type: None,
                        });
                    }
                    Some(multiple_choice) => multiple_choice.as_object().ok_or_else(|| {
                        AIApiError::Other(anyhow::anyhow!(
                            "ask_user_question `multiple_choice` must be an object or null"
                        ))
                    })?,
                };
                let multiple_choice_value = Value::Object(multiple_choice.clone());
                let options = array(&multiple_choice_value, "options")?;
                if options.is_empty() {
                    return Err(AIApiError::Other(anyhow::anyhow!(
                        "multiple_choice requires at least one option"
                    )));
                }
                let recommended_option_index =
                    optional_i64_strict(&multiple_choice_value, "recommended_option_index")?
                        .unwrap_or(0);
                if recommended_option_index < 0 || recommended_option_index >= options.len() as i64
                {
                    return Err(AIApiError::Other(anyhow::anyhow!(
                        "recommended_option_index must point to an available option"
                    )));
                }
                Ok(api::ask_user_question::Question {
                    question_id: required_string(question, "question_id")?,
                    question: required_string(question, "question")?,
                    question_type: Some(
                        api::ask_user_question::question::QuestionType::MultipleChoice(
                            api::ask_user_question::MultipleChoice {
                                options: options
                                    .iter()
                                    .map(|option| {
                                        Ok(api::ask_user_question::Option {
                                            label: required_string(option, "label")?,
                                        })
                                    })
                                    .collect::<Result<Vec<_>, AIApiError>>()?,
                                recommended_option_index: recommended_option_index as i32,
                                is_multiselect: optional_bool_strict(
                                    &multiple_choice_value,
                                    "is_multiselect",
                                )?
                                .unwrap_or(false),
                                supports_other: optional_bool_strict(
                                    &multiple_choice_value,
                                    "supports_other",
                                )?
                                .unwrap_or(false),
                            },
                        ),
                    ),
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
    })
}

fn use_computer_arg(args: &Value) -> Result<api::message::tool_call::UseComputer, AIApiError> {
    let actions = array(args, "actions")?;
    if actions.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "use_computer requires at least one action"
        )));
    }
    Ok(api::message::tool_call::UseComputer {
        actions: actions
            .iter()
            .map(use_computer_action_arg)
            .collect::<Result<Vec<_>, AIApiError>>()?,
        post_actions_screenshot_params: optional_value_strict(
            args,
            "post_actions_screenshot_params",
        )?
        .map(parse_screenshot_params)
        .transpose()?,
        action_summary: optional_string_strict(args, "action_summary")?.unwrap_or_default(),
    })
}

fn use_computer_action_arg(
    value: &Value,
) -> Result<api::message::tool_call::use_computer::Action, AIApiError> {
    use api::message::tool_call::use_computer::action::{self, Type};

    let action_type = required_string(value, "type")?.to_ascii_lowercase();
    let action = match action_type.as_str() {
        "mouse_move" => Type::MouseMove(action::MouseMove {
            to: Some(parse_coordinates(value, "to")?),
        }),
        "mouse_down" => Type::MouseDown(action::MouseDown {
            button: parse_mouse_button(value, "button")?,
            at: Some(parse_coordinates(value, "at")?),
        }),
        "mouse_up" => Type::MouseUp(action::MouseUp {
            button: parse_mouse_button(value, "button")?,
        }),
        "mouse_wheel" => Type::MouseWheel(action::MouseWheel {
            at: Some(parse_coordinates(value, "at")?),
            direction: parse_scroll_direction(value, "direction")?,
            distance: Some(parse_scroll_distance(value)?),
        }),
        "wait" => Type::Wait(action::Wait {
            duration: Some(parse_required_duration(value, "seconds")?),
        }),
        "type_text" => Type::TypeText(action::TypeText {
            text: required_string(value, "text")?,
        }),
        "key_down" => Type::KeyDown(action::KeyDown {
            key: Some(parse_computer_key(value)?),
        }),
        "key_up" => Type::KeyUp(action::KeyUp {
            key: Some(parse_computer_key(value)?),
        }),
        _ => {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "unsupported computer action `{action_type}`"
            )));
        }
    };
    Ok(api::message::tool_call::use_computer::Action {
        r#type: Some(action),
    })
}

fn parse_coordinates(value: &Value, key: &str) -> Result<api::Coordinates, AIApiError> {
    let value = value
        .get(key)
        .filter(|value| !value.is_null())
        .ok_or_else(|| AIApiError::Other(anyhow::anyhow!("missing object argument `{key}`")))?;
    if !value.is_object() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "argument `{key}` must be an object"
        )));
    }
    Ok(api::Coordinates {
        x: required_i32(value, "x")?,
        y: required_i32(value, "y")?,
    })
}

fn parse_mouse_button(value: &Value, key: &str) -> Result<i32, AIApiError> {
    let value = required_string(value, key)?.to_ascii_uppercase();
    let button = match value.as_str() {
        "LEFT" => api::message::tool_call::use_computer::action::MouseButton::Left,
        "RIGHT" => api::message::tool_call::use_computer::action::MouseButton::Right,
        "MIDDLE" => api::message::tool_call::use_computer::action::MouseButton::Middle,
        "BACK" => api::message::tool_call::use_computer::action::MouseButton::Back,
        "FORWARD" => api::message::tool_call::use_computer::action::MouseButton::Forward,
        _ => {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "unsupported mouse button `{value}`"
            )));
        }
    };
    Ok(button as i32)
}

fn parse_scroll_direction(value: &Value, key: &str) -> Result<i32, AIApiError> {
    let value = required_string(value, key)?.to_ascii_uppercase();
    let direction = match value.as_str() {
        "UP" => api::message::tool_call::use_computer::action::mouse_wheel::Direction::Up,
        "DOWN" => api::message::tool_call::use_computer::action::mouse_wheel::Direction::Down,
        "LEFT" => api::message::tool_call::use_computer::action::mouse_wheel::Direction::Left,
        "RIGHT" => api::message::tool_call::use_computer::action::mouse_wheel::Direction::Right,
        _ => {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "unsupported scroll direction `{value}`"
            )));
        }
    };
    Ok(direction as i32)
}

fn parse_scroll_distance(
    value: &Value,
) -> Result<api::message::tool_call::use_computer::action::mouse_wheel::Distance, AIApiError> {
    let distance = value.get("distance").ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!("mouse_wheel requires a distance object"))
    })?;
    let distance = distance.as_object().ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!("mouse_wheel distance must be an object"))
    })?;
    let distance_value = Value::Object(distance.clone());
    let pixels = optional_i32_strict(&distance_value, "pixels")?;
    let clicks = optional_i32_strict(&distance_value, "clicks")?;
    match (pixels, clicks) {
        (Some(pixels), None) => Ok(
            api::message::tool_call::use_computer::action::mouse_wheel::Distance::Pixels(pixels),
        ),
        (None, Some(clicks)) => Ok(
            api::message::tool_call::use_computer::action::mouse_wheel::Distance::Clicks(clicks),
        ),
        (Some(_), Some(_)) => Err(AIApiError::Other(anyhow::anyhow!(
            "mouse_wheel distance must contain exactly one of pixels or clicks"
        ))),
        (None, None) => {
            let kind = optional_string_strict(&distance_value, "kind")?
                .unwrap_or_default()
                .to_ascii_lowercase();
            let amount = required_i32(&distance_value, "value")?;
            match kind.as_str() {
                "pixels" => Ok(
                    api::message::tool_call::use_computer::action::mouse_wheel::Distance::Pixels(
                        amount,
                    ),
                ),
                "clicks" => Ok(
                    api::message::tool_call::use_computer::action::mouse_wheel::Distance::Clicks(
                        amount,
                    ),
                ),
                _ => Err(AIApiError::Other(anyhow::anyhow!(
                    "mouse_wheel distance must contain exactly one of pixels or clicks"
                ))),
            }
        }
    }
}

fn parse_computer_key(
    value: &Value,
) -> Result<api::message::tool_call::use_computer::action::Key, AIApiError> {
    let key = value
        .get("key")
        .filter(|key| !key.is_null())
        .ok_or_else(|| AIApiError::Other(anyhow::anyhow!("key action requires a `key` object")))?;
    if !key.is_object() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "computer key must be an object"
        )));
    }
    let key_value = key.clone();
    let char_value = optional_string_strict(&key_value, "char")?;
    let keycode = optional_i32_strict(&key_value, "keycode")?;
    let data = match (char_value, keycode) {
        (Some(char_value), None) => {
            let mut chars = char_value.chars();
            let ch = chars.next().ok_or_else(|| {
                AIApiError::Other(anyhow::anyhow!("computer key char must not be empty"))
            })?;
            if chars.next().is_some() {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "computer key char must contain exactly one character"
                )));
            }
            api::message::tool_call::use_computer::action::key::Data::Char(ch.to_string())
        }
        (None, Some(keycode)) => {
            api::message::tool_call::use_computer::action::key::Data::Keycode(keycode)
        }
        (Some(_), Some(_)) => {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "computer key must contain exactly one of char or keycode"
            )));
        }
        (None, None) => {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "computer key requires char or keycode"
            )));
        }
    };
    Ok(api::message::tool_call::use_computer::action::Key { data: Some(data) })
}

fn request_computer_use_arg(
    args: &Value,
) -> Result<api::message::tool_call::RequestComputerUse, AIApiError> {
    Ok(api::message::tool_call::RequestComputerUse {
        task_summary: required_string(args, "task_summary")?,
        screenshot_params: optional_value_strict(args, "screenshot_params")?
            .map(parse_screenshot_params)
            .transpose()?,
    })
}

fn parse_screenshot_params(
    value: &Value,
) -> Result<api::message::tool_call::ScreenshotParams, AIApiError> {
    if !value.is_object() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "screenshot_params must be an object"
        )));
    }
    let max_long_edge_px = optional_i32_strict(value, "max_long_edge_px")?.unwrap_or_default();
    let max_total_px = optional_i32_strict(value, "max_total_px")?.unwrap_or_default();
    if max_long_edge_px < 0 || max_total_px < 0 {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "screenshot size limits must not be negative"
        )));
    }
    let region = optional_value_strict(value, "region")?
        .map(|region| -> Result<_, AIApiError> {
            if !region.is_object() {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "screenshot region must be an object"
                )));
            }
            let top_left = optional_coordinates(region, "top_left")?;
            let bottom_right = optional_coordinates(region, "bottom_right")?;
            match (top_left, bottom_right) {
                (Some(top_left), Some(bottom_right)) => {
                    Ok(api::message::tool_call::screenshot_params::Region {
                        top_left: Some(top_left),
                        bottom_right: Some(bottom_right),
                    })
                }
                _ => Err(AIApiError::Other(anyhow::anyhow!(
                    "screenshot region requires complete top_left and bottom_right coordinates"
                ))),
            }
        })
        .transpose()?;
    Ok(api::message::tool_call::ScreenshotParams {
        max_long_edge_px,
        max_total_px,
        region,
    })
}

fn parse_optional_duration(
    value: &Value,
    key: &str,
) -> Result<Option<prost_types::Duration>, AIApiError> {
    let Some(value) = optional_value_strict(value, key)? else {
        return Ok(None);
    };
    let (seconds, nanos) = if let Some(seconds) = value.as_i64() {
        (seconds, 0)
    } else {
        if !value.is_object() {
            return Err(AIApiError::Other(anyhow::anyhow!(
                "duration argument `{key}` must be an integer or object"
            )));
        }
        let seconds = optional_i64_strict(value, "seconds")?.ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!(
                "duration object `{key}` requires integer `seconds`"
            ))
        })?;
        let nanos = optional_i32_strict(value, "nanos")?.unwrap_or_default();
        (seconds, nanos)
    };
    if seconds < 0 {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "duration seconds must not be negative"
        )));
    }
    if !(0..1_000_000_000).contains(&nanos) {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "duration nanos must be between 0 and 999999999"
        )));
    }
    Ok(Some(prost_types::Duration { seconds, nanos }))
}

fn parse_required_duration(value: &Value, key: &str) -> Result<prost_types::Duration, AIApiError> {
    parse_optional_duration(value, key)?.ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!(
            "duration argument `{key}` is required and must not be null"
        ))
    })
}

fn optional_coordinates(value: &Value, key: &str) -> Result<Option<api::Coordinates>, AIApiError> {
    let Some(value) = optional_value_strict(value, key)? else {
        return Ok(None);
    };
    if !value.is_object() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "argument `{key}` must be an object"
        )));
    }
    Ok(Some(api::Coordinates {
        x: required_i32(value, "x")?,
        y: required_i32(value, "y")?,
    }))
}

fn required_i32(value: &Value, key: &str) -> Result<i32, AIApiError> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| AIApiError::Other(anyhow::anyhow!("missing integer argument `{key}`")))
}

fn required_u32(value: &Value, key: &str) -> Result<u32, AIApiError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!("missing unsigned integer argument `{key}`"))
        })
}

fn tool_type_for_openai_name(name: &str) -> Option<api::ToolType> {
    Some(match name {
        "run_shell_command" => api::ToolType::RunShellCommand,
        "read_files" => api::ToolType::ReadFiles,
        "search_codebase" => api::ToolType::SearchCodebase,
        "grep" => api::ToolType::Grep,
        "file_glob" => api::ToolType::FileGlobV2,
        "apply_file_diffs" => api::ToolType::ApplyFileDiffs,
        "read_mcp_resource" => api::ToolType::ReadMcpResource,
        "call_mcp_tool" => api::ToolType::CallMcpTool,
        "read_skill" => api::ToolType::ReadSkill,
        "write_to_long_running_shell_command" => api::ToolType::WriteToLongRunningShellCommand,
        "read_shell_command_output" => api::ToolType::ReadShellCommandOutput,
        "transfer_shell_command_control_to_user" => {
            api::ToolType::TransferShellCommandControlToUser
        }
        "suggest_new_conversation" => api::ToolType::SuggestNewConversation,
        "open_code_review" => api::ToolType::OpenCodeReview,
        "init_project" => api::ToolType::InitProject,
        "read_documents" => api::ToolType::ReadDocuments,
        "edit_documents" => api::ToolType::EditDocuments,
        "create_documents" => api::ToolType::CreateDocuments,
        "insert_review_comments" => api::ToolType::InsertReviewComments,
        "fetch_conversation" => api::ToolType::FetchConversation,
        "ask_user_question" => api::ToolType::AskUserQuestion,
        "use_computer" => api::ToolType::UseComputer,
        "request_computer_use" => api::ToolType::RequestComputerUse,
        _ => return None,
    })
}

fn tool_type_is_advertised(tool_type: api::ToolType, supported_tools: &[api::ToolType]) -> bool {
    if matches!(tool_type, api::ToolType::FileGlobV2) {
        supported_tools.contains(&api::ToolType::FileGlobV2)
            || supported_tools.contains(&api::ToolType::FileGlob)
    } else {
        supported_tools.contains(&tool_type)
    }
}

fn supports_complete_long_running_shell_family(supported_tools: &[api::ToolType]) -> bool {
    [
        api::ToolType::RunShellCommand,
        api::ToolType::WriteToLongRunningShellCommand,
        api::ToolType::ReadShellCommandOutput,
        api::ToolType::TransferShellCommandControlToUser,
    ]
    .into_iter()
    .all(|tool| supported_tools.contains(&tool))
}

#[cfg(test)]
#[allow(deprecated)]
fn openai_tool_name(tool: &api::message::tool_call::Tool) -> &'static str {
    match tool {
        api::message::tool_call::Tool::RunShellCommand(_) => "run_shell_command",
        api::message::tool_call::Tool::ReadFiles(_) => "read_files",
        api::message::tool_call::Tool::SearchCodebase(_) => "search_codebase",
        api::message::tool_call::Tool::Grep(_) => "grep",
        api::message::tool_call::Tool::FileGlob(_)
        | api::message::tool_call::Tool::FileGlobV2(_) => "file_glob",
        api::message::tool_call::Tool::ApplyFileDiffs(_) => "apply_file_diffs",
        api::message::tool_call::Tool::ReadMcpResource(_) => "read_mcp_resource",
        api::message::tool_call::Tool::CallMcpTool(_) => "call_mcp_tool",
        api::message::tool_call::Tool::ReadSkill(_) => "read_skill",
        api::message::tool_call::Tool::WriteToLongRunningShellCommand(_) => {
            "write_to_long_running_shell_command"
        }
        api::message::tool_call::Tool::ReadShellCommandOutput(_) => "read_shell_command_output",
        api::message::tool_call::Tool::TransferShellCommandControlToUser(_) => {
            "transfer_shell_command_control_to_user"
        }
        api::message::tool_call::Tool::SuggestNewConversation(_) => "suggest_new_conversation",
        api::message::tool_call::Tool::OpenCodeReview(_) => "open_code_review",
        api::message::tool_call::Tool::InitProject(_) => "init_project",
        api::message::tool_call::Tool::ReadDocuments(_) => "read_documents",
        api::message::tool_call::Tool::EditDocuments(_) => "edit_documents",
        api::message::tool_call::Tool::CreateDocuments(_) => "create_documents",
        api::message::tool_call::Tool::InsertReviewComments(_) => "insert_review_comments",
        api::message::tool_call::Tool::FetchConversation(_) => "fetch_conversation",
        api::message::tool_call::Tool::AskUserQuestion(_) => "ask_user_question",
        api::message::tool_call::Tool::UseComputer(_) => "use_computer",
        api::message::tool_call::Tool::RequestComputerUse(_) => "request_computer_use",
        _ => "unsupported",
    }
}

#[cfg(test)]
fn run_shell_wait_value(tool: &api::message::tool_call::Tool) -> bool {
    let api::message::tool_call::Tool::RunShellCommand(command) = tool else {
        panic!("expected run_shell_command");
    };
    matches!(
        command.wait_until_complete_value,
        Some(
            api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(
                true
            )
        )
    )
}

struct InferredShellCommandFlags {
    is_read_only: bool,
    is_risky: bool,
}

fn infer_shell_command_flags(command: &str) -> InferredShellCommandFlags {
    let command = command.trim();
    let first_word = command.split_whitespace().next().unwrap_or_default();
    let is_read_only = if command.contains('>') || command.contains(">>") {
        false
    } else if matches!(
        first_word,
        "pwd"
            | "ls"
            | "cat"
            | "head"
            | "tail"
            | "grep"
            | "rg"
            | "find"
            | "fd"
            | "which"
            | "type"
            | "wc"
            | "stat"
    ) {
        true
    } else if command.starts_with("sed -n ") {
        true
    } else {
        let mut words = command.split_whitespace();
        matches!(
            (words.next(), words.next()),
            (
                Some("git"),
                Some(
                    "status"
                        | "diff"
                        | "log"
                        | "show"
                        | "branch"
                        | "rev-parse"
                        | "grep"
                        | "ls-files"
                        | "remote"
                )
            )
        )
    };

    InferredShellCommandFlags {
        is_read_only,
        is_risky: matches!(
            first_word,
            "rm" | "curl" | "wget" | "ssh" | "scp" | "rsync" | "eval" | "exec"
        ),
    }
}

fn read_file_arg(value: &Value) -> Result<api::message::tool_call::read_files::File, AIApiError> {
    let line_ranges = optional_array_strict(value, "line_ranges")?
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|range| {
            let start = required_u32(range, "start")?;
            let end = required_u32(range, "end")?;
            if start > end {
                return Err(AIApiError::Other(anyhow::anyhow!(
                    "line range `start` must not exceed `end`"
                )));
            }
            Ok(api::FileContentLineRange { start, end })
        })
        .collect::<Result<Vec<_>, AIApiError>>()?;
    Ok(api::message::tool_call::read_files::File {
        name: required_string_from_alias(value, &["name", "path"])?,
        line_ranges,
    })
}

fn apply_file_diffs_arg(
    args: &Value,
) -> Result<api::message::tool_call::ApplyFileDiffs, AIApiError> {
    let diffs = optional_array_strict(args, "diffs")?
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let new_files = optional_array_strict(args, "new_files")?
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let deleted_files = optional_array_strict(args, "deleted_files")?
        .or(optional_array_strict(args, "delete_files")?)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    Ok(api::message::tool_call::ApplyFileDiffs {
        summary: optional_string_strict(args, "summary")?.unwrap_or_default(),
        diffs: diffs
            .iter()
            .map(|diff| {
                Ok(api::message::tool_call::apply_file_diffs::FileDiff {
                    file_path: required_string_from_alias(diff, &["file_path", "path"])?,
                    search: required_string(diff, "search")?,
                    replace: required_string(diff, "replace")?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
        new_files: new_files
            .iter()
            .map(|file| {
                Ok(api::message::tool_call::apply_file_diffs::NewFile {
                    file_path: required_string_from_alias(file, &["file_path", "path"])?,
                    content: required_string(file, "content")?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
        deleted_files: deleted_files
            .iter()
            .map(|file| {
                Ok(api::message::tool_call::apply_file_diffs::DeleteFile {
                    file_path: required_string_from_alias(file, &["file_path", "path"])?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
        v4a_updates: vec![],
    })
}

fn required_string(value: &Value, key: &str) -> Result<String, AIApiError> {
    optional_string_strict(value, key)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AIApiError::Other(anyhow::anyhow!("missing string argument `{key}`")))
}

fn required_string_from_alias(value: &Value, keys: &[&str]) -> Result<String, AIApiError> {
    for key in keys {
        if optional_value_strict(value, key)?.is_some() {
            return required_string(value, key);
        }
    }
    Err(AIApiError::Other(anyhow::anyhow!(
        "missing string argument `{}`",
        keys.join("` or `")
    )))
}

fn optional_value_strict<'a>(value: &'a Value, key: &str) -> Result<Option<&'a Value>, AIApiError> {
    Ok(match value.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => Some(value),
    })
}

fn optional_string_strict(value: &Value, key: &str) -> Result<Option<String>, AIApiError> {
    let Some(value) = optional_value_strict(value, key)? else {
        return Ok(None);
    };
    value
        .as_str()
        .map(ToString::to_string)
        .map(Some)
        .ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!(
                "optional argument `{key}` must be a string or null"
            ))
        })
}

fn optional_bool_strict(value: &Value, key: &str) -> Result<Option<bool>, AIApiError> {
    let Some(value) = optional_value_strict(value, key)? else {
        return Ok(None);
    };
    value.as_bool().map(Some).ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!(
            "optional argument `{key}` must be a boolean or null"
        ))
    })
}

fn optional_i64_strict(value: &Value, key: &str) -> Result<Option<i64>, AIApiError> {
    let Some(value) = optional_value_strict(value, key)? else {
        return Ok(None);
    };
    value.as_i64().map(Some).ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!(
            "optional argument `{key}` must be an integer or null"
        ))
    })
}

fn optional_i32_strict(value: &Value, key: &str) -> Result<Option<i32>, AIApiError> {
    let Some(value) = optional_i64_strict(value, key)? else {
        return Ok(None);
    };
    i32::try_from(value).map(Some).map_err(|_| {
        AIApiError::Other(anyhow::anyhow!(
            "optional argument `{key}` is outside the signed 32-bit range"
        ))
    })
}

fn nonnegative_optional_i32(value: &Value, key: &str) -> Result<Option<i32>, AIApiError> {
    let Some(value) = optional_i32_strict(value, key)? else {
        return Ok(None);
    };
    if value < 0 {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "optional argument `{key}` must not be negative"
        )));
    }
    Ok(Some(value))
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, AIApiError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| AIApiError::Other(anyhow::anyhow!("missing array argument `{key}`")))
}

fn optional_array_strict<'a>(
    value: &'a Value,
    key: &str,
) -> Result<Option<&'a Vec<Value>>, AIApiError> {
    let Some(value) = optional_value_strict(value, key)? else {
        return Ok(None);
    };
    value.as_array().map(Some).ok_or_else(|| {
        AIApiError::Other(anyhow::anyhow!(
            "optional argument `{key}` must be an array or null"
        ))
    })
}

fn optional_string_array_strict(
    value: &Value,
    key: &str,
) -> Result<Option<Vec<String>>, AIApiError> {
    let Some(values) = optional_array_strict(value, key)? else {
        return Ok(None);
    };
    values
        .iter()
        .map(|value| {
            value.as_str().map(ToString::to_string).ok_or_else(|| {
                AIApiError::Other(anyhow::anyhow!(
                    "optional array argument `{key}` must contain only strings"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn string_array_or_single(
    value: &Value,
    array_key: &str,
    single_key: &str,
) -> Result<Vec<String>, AIApiError> {
    if let Some(values) = optional_string_array_strict(value, array_key)? {
        if !values.is_empty() {
            return Ok(values);
        }
    }
    required_string(value, single_key).map(|value| vec![value])
}

fn prost_struct_to_json(value: &prost_types::Struct) -> Value {
    Value::Object(
        value
            .fields
            .iter()
            .map(|(key, value)| (key.clone(), prost_value_to_json(value)))
            .collect(),
    )
}

fn prost_value_to_json(value: &prost_types::Value) -> Value {
    use prost_types::value::Kind;
    match value.kind.as_ref() {
        Some(Kind::NullValue(_)) | None => Value::Null,
        Some(Kind::NumberValue(value)) => json!(value),
        Some(Kind::StringValue(value)) => json!(value),
        Some(Kind::BoolValue(value)) => json!(value),
        Some(Kind::StructValue(value)) => prost_struct_to_json(value),
        Some(Kind::ListValue(value)) => {
            Value::Array(value.values.iter().map(prost_value_to_json).collect())
        }
    }
}

fn serde_json_to_prost(value: Value) -> Result<prost_types::Value, String> {
    use prost_types::value::Kind::*;
    use serde_json::Value::*;

    Ok(prost_types::Value {
        kind: Some(match value {
            Null => NullValue(0),
            Bool(value) => BoolValue(value),
            Number(value) => NumberValue(
                value
                    .as_f64()
                    .ok_or_else(|| format!("float {value} is not a valid JSON number"))?,
            ),
            String(value) => StringValue(value),
            Array(values) => ListValue(prost_types::ListValue {
                values: values
                    .into_iter()
                    .map(serde_json_to_prost)
                    .collect::<Result<Vec<_>, std::string::String>>()?,
            }),
            Object(values) => StructValue(prost_types::Struct {
                fields: values
                    .into_iter()
                    .map(|(key, value)| serde_json_to_prost(value).map(|value| (key, value)))
                    .collect::<Result<BTreeMap<_, _>, std::string::String>>()?,
            }),
        }),
    })
}

fn tool_call_result_to_text_with_tool_policy(
    result: &api::message::ToolCallResult,
    long_running_shell_controls_advertised: bool,
) -> String {
    if !long_running_shell_controls_advertised
        && result
            .result
            .as_ref()
            .is_some_and(long_running_shell_result_is_snapshot)
    {
        return serde_json::to_string(&json!({
            "schema": "warp.direct_openai.tool_result",
            "version": 1,
            "tool_call_id": result.tool_call_id,
            "result_type": result
                .result
                .as_ref()
                .map(tool_call_result_type_name)
                .unwrap_or("none"),
            "status": "unavailable",
            "message": "Long-running shell output is unavailable in this request because its polling and control tools are not advertised."
        }))
        .unwrap_or_else(|_| "{\"schema\":\"warp.direct_openai.tool_result\",\"version\":1,\"status\":\"unavailable\"}".to_string());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(result.encode_to_vec());
    serde_json::to_string(&json!({
        "schema": "warp.direct_openai.tool_result",
        "version": 1,
        "tool_call_id": result.tool_call_id,
        "result_type": result
            .result
            .as_ref()
            .map(tool_call_result_type_name)
            .unwrap_or("none"),
        "protobuf_base64": encoded,
    }))
    .unwrap_or_else(|_| {
        "{\"schema\":\"warp.direct_openai.tool_result\",\"version\":1,\"result_type\":\"none\"}"
            .to_string()
    })
}

fn tool_call_result_type_name(result: &api::message::tool_call_result::Result) -> &'static str {
    use api::message::tool_call_result::Result as ToolCallResultType;
    match result {
        ToolCallResultType::RunShellCommand(_) => "run_shell_command",
        ToolCallResultType::SearchCodebase(_) => "search_codebase",
        ToolCallResultType::Server(_) => "server",
        ToolCallResultType::ReadFiles(_) => "read_files",
        ToolCallResultType::ApplyFileDiffs(_) => "apply_file_diffs",
        ToolCallResultType::SuggestPlan(_) => "suggest_plan",
        ToolCallResultType::SuggestCreatePlan(_) => "suggest_create_plan",
        ToolCallResultType::Grep(_) => "grep",
        ToolCallResultType::FileGlob(_) => "file_glob",
        ToolCallResultType::Cancel(_) => "cancel",
        ToolCallResultType::ReadMcpResource(_) => "read_mcp_resource",
        ToolCallResultType::CallMcpTool(_) => "call_mcp_tool",
        ToolCallResultType::WriteToLongRunningShellCommand(_) => {
            "write_to_long_running_shell_command"
        }
        ToolCallResultType::SuggestNewConversation(_) => "suggest_new_conversation",
        ToolCallResultType::FileGlobV2(_) => "file_glob",
        ToolCallResultType::SuggestPrompt(_) => "suggest_prompt",
        ToolCallResultType::OpenCodeReview(_) => "open_code_review",
        ToolCallResultType::InitProject(_) => "init_project",
        ToolCallResultType::Subagent(_) => "subagent",
        ToolCallResultType::ReadDocuments(_) => "read_documents",
        ToolCallResultType::EditDocuments(_) => "edit_documents",
        ToolCallResultType::CreateDocuments(_) => "create_documents",
        ToolCallResultType::ReadShellCommandOutput(_) => "read_shell_command_output",
        ToolCallResultType::UseComputer(_) => "use_computer",
        ToolCallResultType::InsertReviewComments(_) => "insert_review_comments",
        ToolCallResultType::ReadSkill(_) => "read_skill",
        ToolCallResultType::RequestComputerUseResult(_) => "request_computer_use",
        ToolCallResultType::FetchConversation(_) => "fetch_conversation",
        ToolCallResultType::StartAgent(_) => "start_agent",
        ToolCallResultType::SendMessageToAgent(_) => "send_message_to_agent",
        ToolCallResultType::TransferShellCommandControlToUser(_) => {
            "transfer_shell_command_control_to_user"
        }
        ToolCallResultType::AskUserQuestion(_) => "ask_user_question",
        ToolCallResultType::StartAgentV2(_) => "start_agent_v2",
        ToolCallResultType::UploadFileArtifact(_) => "upload_file_artifact",
        ToolCallResultType::RunAgentsResult(_) => "run_agents_result",
    }
}

fn long_running_shell_result_is_snapshot(result: &api::message::tool_call_result::Result) -> bool {
    match result {
        api::message::tool_call_result::Result::RunShellCommand(result) => matches!(
            result.result.as_ref(),
            Some(api::run_shell_command_result::Result::LongRunningCommandSnapshot(_))
        ),
        api::message::tool_call_result::Result::WriteToLongRunningShellCommand(result) => {
            matches!(
                result.result.as_ref(),
                Some(
                    api::write_to_long_running_shell_command_result::Result::LongRunningCommandSnapshot(_)
                )
            )
        }
        api::message::tool_call_result::Result::ReadShellCommandOutput(result) => matches!(
            result.result.as_ref(),
            Some(api::read_shell_command_output_result::Result::LongRunningCommandSnapshot(_))
        ),
        api::message::tool_call_result::Result::TransferShellCommandControlToUser(result) => {
            matches!(
                result.result.as_ref(),
                Some(
                    api::transfer_shell_command_control_to_user_result::Result::LongRunningCommandSnapshot(_)
                )
            )
        }
        _ => false,
    }
}

fn add_messages_action(task_id: &str, messages: Vec<api::Message>) -> api::ClientAction {
    api::ClientAction {
        action: Some(api::client_action::Action::AddMessagesToTask(
            api::client_action::AddMessagesToTask {
                task_id: task_id.to_string(),
                messages,
            },
        )),
    }
}

fn agent_output_message(
    task_id: &str,
    request_id: &str,
    message_id: String,
    text: String,
) -> api::Message {
    api::Message {
        id: message_id,
        task_id: task_id.to_string(),
        request_id: request_id.to_string(),
        timestamp: None,
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput { text },
        )),
    }
}

fn append_agent_output_event(task_id: &str, message_id: &str, delta: String) -> api::ResponseEvent {
    client_actions_event(vec![api::ClientAction {
        action: Some(api::client_action::Action::AppendToMessageContent(
            api::client_action::AppendToMessageContent {
                task_id: task_id.to_string(),
                message: Some(api::Message {
                    id: message_id.to_string(),
                    task_id: task_id.to_string(),
                    request_id: String::new(),
                    timestamp: None,
                    server_message_data: String::new(),
                    citations: vec![],
                    message: Some(api::message::Message::AgentOutput(
                        api::message::AgentOutput { text: delta },
                    )),
                }),
                mask: Some(prost_types::FieldMask {
                    paths: vec!["agent_output.text".to_string()],
                }),
            },
        )),
    }])
}

fn client_actions_event(actions: Vec<api::ClientAction>) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions { actions },
        )),
    }
}

fn finished_event_for_openai_finish_reason(
    finish_reason: Option<&str>,
    tool_call_count: usize,
) -> api::ResponseEvent {
    let reason = match finish_reason {
        Some("length") => {
            api::response_event::stream_finished::Reason::InternalError(
                api::response_event::stream_finished::InternalError {
                    message: "OpenAI-compatible provider stopped because the output token limit was reached.".to_string(),
                },
            )
        }
        Some("content_filter") => {
            api::response_event::stream_finished::Reason::InternalError(
                api::response_event::stream_finished::InternalError {
                    message: "OpenAI-compatible provider stopped because its content filter interrupted the response.".to_string(),
                },
            )
        }
        Some(other)
            if !matches!(other, "stop" | "tool_calls" | "function_call")
                && tool_call_count == 0 =>
        {
            log::warn!(
                "OpenAI-compatible provider returned unrecognized finish_reason without tool calls: {other}"
            );
            api::response_event::stream_finished::Reason::Done(
                api::response_event::stream_finished::Done {},
            )
        }
        _ => api::response_event::stream_finished::Reason::Done(
            api::response_event::stream_finished::Done {},
        ),
    };

    finished_event(reason)
}

fn finished_event(reason: api::response_event::stream_finished::Reason) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::Finished(
            api::response_event::StreamFinished {
                token_usage: vec![],
                should_refresh_model_config: false,
                request_cost: None,
                conversation_usage_metadata: None,
                reason: Some(reason),
            },
        )),
    }
}

pub(super) fn error_stream(message: impl Into<String>) -> ResponseStream {
    let message = message.into();
    Box::pin(stream! {
        yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(message))));
    })
}

fn i32_json_schema(minimum: i64, maximum: i64) -> Value {
    json!({
        "type": "integer",
        "minimum": minimum,
        "maximum": maximum,
    })
}

fn coordinates_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "x": i32_json_schema(i32::MIN as i64, i32::MAX as i64),
            "y": i32_json_schema(i32::MIN as i64, i32::MAX as i64),
        },
        "required": ["x", "y"],
        "additionalProperties": false,
    })
}

fn screenshot_params_json_schema() -> Value {
    let coordinates = coordinates_json_schema();
    json!({
        "type": "object",
        "properties": {
            "max_long_edge_px": i32_json_schema(0, i32::MAX as i64),
            "max_total_px": i32_json_schema(0, i32::MAX as i64),
            "region": {
                "oneOf": [
                    {"type": "null"},
                    {
                        "type": "object",
                        "properties": {
                            "top_left": coordinates,
                            "bottom_right": coordinates_json_schema(),
                        },
                        "required": ["top_left", "bottom_right"],
                        "additionalProperties": false,
                    }
                ]
            },
        },
        "additionalProperties": false,
    })
}

fn duration_json_schema() -> Value {
    json!({
        "oneOf": [
            i32_json_schema(0, i64::MAX),
            {
                "type": "object",
                "properties": {
                    "seconds": {"type":"integer", "minimum":0, "maximum":i64::MAX},
                    "nanos": {"type":"integer", "minimum":0, "maximum":999999999},
                },
                "required": ["seconds"],
                "additionalProperties": false,
            }
        ]
    })
}

fn computer_key_json_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {"char": {"type":"string", "minLength":1, "maxLength":1}},
                "required": ["char"],
                "additionalProperties": false,
            },
            {
                "type": "object",
                "properties": {"keycode": i32_json_schema(i32::MIN as i64, i32::MAX as i64)},
                "required": ["keycode"],
                "additionalProperties": false,
            }
        ]
    })
}

fn mouse_wheel_distance_json_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {"pixels": i32_json_schema(i32::MIN as i64, i32::MAX as i64)},
                "required": ["pixels"],
                "additionalProperties": false,
            },
            {
                "type": "object",
                "properties": {"clicks": i32_json_schema(i32::MIN as i64, i32::MAX as i64)},
                "required": ["clicks"],
                "additionalProperties": false,
            }
        ]
    })
}

fn computer_action_json_schema() -> Value {
    let coordinates = coordinates_json_schema();
    let duration = duration_json_schema();
    let key = computer_key_json_schema();
    let distance = mouse_wheel_distance_json_schema();
    json!({
        "oneOf": [
            {
                "type":"object",
                "properties": {"type":{"const":"mouse_move"}, "to":coordinates},
                "required":["type", "to"],
                "additionalProperties":false,
            },
            {
                "type":"object",
                "properties": {"type":{"const":"mouse_down"}, "button":{"type":"string","enum":["left","right","middle","back","forward"]}, "at":coordinates_json_schema()},
                "required":["type", "button", "at"],
                "additionalProperties":false,
            },
            {
                "type":"object",
                "properties": {"type":{"const":"mouse_up"}, "button":{"type":"string","enum":["left","right","middle","back","forward"]}},
                "required":["type", "button"],
                "additionalProperties":false,
            },
            {
                "type":"object",
                "properties": {"type":{"const":"mouse_wheel"}, "at":coordinates_json_schema(), "direction":{"type":"string","enum":["up","down","left","right"]}, "distance":distance},
                "required":["type", "at", "direction", "distance"],
                "additionalProperties":false,
            },
            {
                "type":"object",
                "properties": {"type":{"const":"wait"}, "seconds":duration},
                "required":["type", "seconds"],
                "additionalProperties":false,
            },
            {
                "type":"object",
                "properties": {"type":{"const":"type_text"}, "text":{"type":"string"}},
                "required":["type", "text"],
                "additionalProperties":false,
            },
            {
                "type":"object",
                "properties": {"type":{"const":"key_down"}, "key":key},
                "required":["type", "key"],
                "additionalProperties":false,
            },
            {
                "type":"object",
                "properties": {"type":{"const":"key_up"}, "key":computer_key_json_schema()},
                "required":["type", "key"],
                "additionalProperties":false,
            },
        ]
    })
}

fn openai_tools_for_supported_tools(supported_tools: &[api::ToolType]) -> Vec<OpenAITool> {
    let supported = supported_tools.iter().copied().collect::<HashSet<_>>();
    let mut tools = Vec::new();

    if supported.contains(&api::ToolType::RunShellCommand) {
        let wait_description = if supports_complete_long_running_shell_family(supported_tools) {
            "Set false only for a command that needs the long-running shell controls; the direct adapter advertises write, output, and control-transfer tools for this request."
        } else {
            "Use true. The OpenAI-compatible direct provider path waits for command completion before returning the tool result."
        };
        tools.push(openai_tool(
            "run_shell_command",
            "Run a shell command in the user's Warp terminal. Use this instead of giving shell commands as instructions when execution is needed.",
            json_schema_object(
                [
                    (
                        "command",
                        json!({
                            "type": "string",
                            "description": "The exact command to run. Prefer direct commands over shell wrappers."
                        }),
                    ),
                    (
                        "is_read_only",
                        json!({
                            "type": "boolean",
                            "description": "True only when the command inspects state without modifying files, processes, network state, secrets, or external services."
                        }),
                    ),
                    (
                        "uses_pager",
                        json!({
                            "type": "boolean",
                            "description": "True if the command might invoke an interactive pager such as less."
                        }),
                    ),
                    (
                        "is_risky",
                        json!({
                            "type": "boolean",
                            "description": "False for ordinary inspection, build, and test commands; true for destructive, credential-changing, network-sensitive, or externally mutating commands."
                        }),
                    ),
                    (
                        "wait_until_completion",
                        json!({
                            "type": "boolean",
                            "description": wait_description
                        }),
                    ),
                ],
                ["command"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::ReadFiles) {
        tools.push(openai_tool(
            "read_files",
            "Read one or more local files. Use absolute paths when available, otherwise paths are resolved relative to the current directory.",
            json_schema_object(
                [(
                    "files",
                    json!({
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "line_ranges": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "start": {"type": "integer"},
                                            "end": {"type": "integer"}
                                        },
                                        "required": ["start", "end"]
                                    }
                                }
                            },
                            "required": ["name"]
                        }
                    }),
                )],
                ["files"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::SearchCodebase) {
        tools.push(openai_tool(
            "search_codebase",
            "Semantic and lexical search over the current codebase.",
            json_schema_object(
                [
                    ("query", json!({"type": "string"})),
                    (
                        "path_filters",
                        json!({"type": "array", "items": {"type": "string"}}),
                    ),
                    ("codebase_path", json!({"type": "string"})),
                ],
                ["query"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::Grep) {
        tools.push(openai_tool(
            "grep",
            "Search file contents using literal text or regex-like queries.",
            json_schema_object(
                [
                    (
                        "queries",
                        json!({"type": "array", "items": {"type": "string"}}),
                    ),
                    ("query", json!({"type": "string"})),
                    ("path", json!({"type": "string"})),
                ],
                [],
            ),
        ));
    }

    if supported.contains(&api::ToolType::FileGlobV2)
        || supported.contains(&api::ToolType::FileGlob)
    {
        tools.push(openai_tool(
            "file_glob",
            "Find files by glob patterns.",
            json_schema_object(
                [
                    (
                        "patterns",
                        json!({"type": "array", "items": {"type": "string"}}),
                    ),
                    ("pattern", json!({"type": "string"})),
                    ("search_dir", json!({"type": "string"})),
                    ("path", json!({"type": "string"})),
                    ("max_matches", json!({"type": "integer"})),
                    ("max_depth", json!({"type": "integer"})),
                    ("min_depth", json!({"type": "integer"})),
                ],
                [],
            ),
        ));
    }

    if supported.contains(&api::ToolType::ApplyFileDiffs) {
        tools.push(openai_tool(
            "apply_file_diffs",
            "Apply targeted file edits. Prefer read_files first, then provide exact search and replace strings.",
            json_schema_object(
                [
                    ("summary", json!({"type": "string"})),
                    (
                        "diffs",
                        json!({
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "file_path": {"type": "string"},
                                    "search": {"type": "string"},
                                    "replace": {"type": "string"}
                                },
                                "required": ["file_path", "search", "replace"]
                            }
                        }),
                    ),
                    (
                        "new_files",
                        json!({
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "file_path": {"type": "string"},
                                    "content": {"type": "string"}
                                },
                                "required": ["file_path", "content"]
                            }
                        }),
                    ),
                    (
                        "deleted_files",
                        json!({
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {"file_path": {"type": "string"}},
                                "required": ["file_path"]
                            }
                        }),
                    ),
                ],
                ["summary"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::ReadMcpResource) {
        tools.push(openai_tool(
            "read_mcp_resource",
            "Read a resource exposed by a configured MCP server.",
            json_schema_object(
                [
                    ("uri", json!({"type": "string"})),
                    ("server_id", json!({"type": "string"})),
                ],
                ["uri"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::CallMcpTool) {
        tools.push(openai_tool(
            "call_mcp_tool",
            "Call a tool exposed by a configured MCP server. Use the MCP context in the system message for names and server ids.",
            json_schema_object(
                [
                    ("name", json!({"type": "string"})),
                    ("server_id", json!({"type": "string"})),
                    ("args", json!({"type": "object"})),
                ],
                ["name", "args"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::ReadSkill) {
        tools.push(openai_tool(
            "read_skill",
            "Read a Warp/Codex skill file before following that skill's instructions.",
            json_schema_object(
                [
                    ("skill_path", json!({"type": "string"})),
                    ("bundled_skill_id", json!({"type": "string"})),
                    ("name", json!({"type": "string"})),
                ],
                [],
            ),
        ));
    }

    if supported.contains(&api::ToolType::WriteToLongRunningShellCommand) {
        tools.push(openai_tool(
            "write_to_long_running_shell_command",
            "Write input to a running local shell command. Use the command_id from its long-running snapshot.",
            json_schema_object(
                [
                    ("command_id", json!({"type": "string"})),
                    (
                        "input",
                        json!({
                            "description": "UTF-8 input text or an array of byte values.",
                            "oneOf": [
                                {"type": "string"},
                                {"type": "array", "items": {"type": "integer", "minimum": 0, "maximum": 255}}
                            ]
                        }),
                    ),
                    (
                        "mode",
                        json!({"type": "string", "enum": ["raw", "line", "block"]}),
                    ),
                ],
                ["command_id", "input"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::ReadShellCommandOutput) {
        tools.push(openai_tool(
            "read_shell_command_output",
            "Read output from a running local shell command by command_id.",
            json_schema_object(
                [
                    ("command_id", json!({"type": "string"})),
                    (
                        "delay",
                        json!({
                            "oneOf": [
                                {"type": "string", "enum": ["on_completion"]},
                                {"type": "object", "properties": {"kind": {"type": "string", "enum": ["duration"]}, "seconds": {"type": "integer", "minimum": 0}}, "required": ["seconds"], "additionalProperties": false}
                            ]
                        }),
                    ),
                    ("delay_seconds", json!({"type": "integer", "minimum": 0})),
                ],
                ["command_id"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::TransferShellCommandControlToUser) {
        tools.push(openai_tool(
            "transfer_shell_command_control_to_user",
            "Transfer control of a running local shell command back to the user with a reason.",
            json_schema_object([("reason", json!({"type": "string"}))], ["reason"]),
        ));
    }

    if supported.contains(&api::ToolType::SuggestNewConversation) {
        tools.push(openai_tool(
            "suggest_new_conversation",
            "Suggest branching a new local conversation from a message.",
            json_schema_object([("message_id", json!({"type": "string"}))], ["message_id"]),
        ));
    }

    if supported.contains(&api::ToolType::OpenCodeReview) {
        tools.push(openai_tool(
            "open_code_review",
            "Open the local code review pane.",
            json_schema_object([], []),
        ));
    }

    if supported.contains(&api::ToolType::InitProject) {
        tools.push(openai_tool(
            "init_project",
            "Open the local project initialization flow.",
            json_schema_object([], []),
        ));
    }

    if supported.contains(&api::ToolType::ReadDocuments) {
        tools.push(openai_tool(
            "read_documents",
            "Read one or more local AI documents by UUID.",
            json_schema_object(
                [(
                    "documents",
                    json!({
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "document_id": {"type": "string", "description": "Document UUID."},
                                "line_ranges": {"type": "array", "items": {"type": "object", "properties": {"start": {"type": "integer", "minimum": 0}, "end": {"type": "integer", "minimum": 0}}, "required": ["start", "end"], "additionalProperties": false}}
                            },
                            "required": ["document_id"],
                            "additionalProperties": false
                        }
                    }),
                )],
                ["documents"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::EditDocuments) {
        tools.push(openai_tool(
            "edit_documents",
            "Apply exact search-and-replace edits to local AI documents.",
            json_schema_object(
                [(
                    "diffs",
                    json!({"type": "array", "items": {"type": "object", "properties": {"document_id": {"type": "string"}, "search": {"type": "string"}, "replace": {"type": "string"}}, "required": ["document_id", "search", "replace"], "additionalProperties": false}}),
                )],
                ["diffs"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::CreateDocuments) {
        tools.push(openai_tool(
            "create_documents",
            "Create one or more local AI documents.",
            json_schema_object(
                [(
                    "documents",
                    json!({"type": "array", "items": {"type": "object", "properties": {"content": {"type": "string"}, "title": {"type": "string"}}, "required": ["content"], "additionalProperties": false}}),
                )],
                ["documents"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::InsertReviewComments) {
        tools.push(openai_tool(
            "insert_review_comments",
            "Insert review comments into the local code review workflow.",
            json_schema_object(
                [
                    ("repo_path", json!({"type": "string"})),
                    ("base_branch", json!({"type": "string"})),
                    (
                        "comments",
                        json!({"type": "array", "items": {"type": "object", "properties": {"comment_id": {"type": "string"}, "author": {"type": "string"}, "last_modified_timestamp": {"type": "string"}, "comment_body": {"type": "string"}, "parent_comment_id": {"type": "string"}, "html_url": {"type": "string"}, "location": {"type": "object", "properties": {"file_path": {"type": "string"}, "line": {"type": "object", "properties": {"diff_hunk": {"type": "string"}, "range": {"type": "object", "properties": {"start": {"type": "integer", "minimum": 0}, "end": {"type": "integer", "minimum": 0}}, "required": ["start", "end"], "additionalProperties": false}, "side": {"type": "string", "enum": ["NEW", "OLD"]}}, "required": ["range"], "additionalProperties": false}}, "required": ["file_path"], "additionalProperties": false}}, "required": ["comment_id", "comment_body"], "additionalProperties": false}}),
                    ),
                ],
                ["repo_path", "comments"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::FetchConversation) {
        tools.push(openai_tool(
            "fetch_conversation",
            "Materialize another local conversation for context.",
            json_schema_object(
                [("conversation_id", json!({"type": "string"}))],
                ["conversation_id"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::AskUserQuestion) {
        tools.push(openai_tool(
            "ask_user_question",
            "Ask the user one or more local multiple-choice questions.",
            json_schema_object(
                [(
                    "questions",
                    json!({"type": "array", "items": {"type": "object", "properties": {"question_id": {"type": "string"}, "question": {"type": "string"}, "multiple_choice": {"type": "object", "properties": {"options": {"type": "array", "items": {"type": "object", "properties": {"label": {"type": "string"}}, "required": ["label"], "additionalProperties": false}}, "recommended_option_index": {"type": "integer", "minimum": 0}, "is_multiselect": {"type": "boolean"}, "supports_other": {"type": "boolean"}}, "required": ["options"], "additionalProperties": false}}, "required": ["question_id", "question", "multiple_choice"], "additionalProperties": false}}),
                )],
                ["questions"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::UseComputer) {
        tools.push(openai_tool(
            "use_computer",
            "Execute approved local computer-use actions.",
            json_schema_object(
                [
                    (
                        "actions",
                        json!({"type": "array", "items": computer_action_json_schema()}),
                    ),
                    ("action_summary", json!({"type": "string"})),
                    (
                        "post_actions_screenshot_params",
                        screenshot_params_json_schema(),
                    ),
                ],
                ["actions"],
            ),
        ));
    }

    if supported.contains(&api::ToolType::RequestComputerUse) {
        tools.push(openai_tool(
            "request_computer_use",
            "Ask for local user approval before computer use.",
            json_schema_object(
                [
                    ("task_summary", json!({"type": "string"})),
                    ("screenshot_params", screenshot_params_json_schema()),
                ],
                ["task_summary"],
            ),
        ));
    }

    tools
}

fn openai_tool(name: &'static str, description: &'static str, parameters: Value) -> OpenAITool {
    OpenAITool {
        kind: "function",
        function: OpenAIToolFunction {
            name,
            description,
            parameters,
        },
    }
}

fn json_schema_object<const N: usize, const M: usize>(
    properties: [(&'static str, Value); N],
    required: [&'static str; M],
) -> Value {
    json!({
        "type": "object",
        "properties": properties
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<serde_json::Map<_, _>>(),
        "required": required
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        "additionalProperties": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::agent::ImageContext;
    use mockito::Matcher;

    #[test]
    fn parses_custom_model_ids_into_provider_and_model() {
        let route = parse_custom_model_id("custom/local-openai/gpt-4o-mini").unwrap();

        assert_eq!(route.provider_name, "local-openai");
        assert_eq!(route.model, "gpt-4o-mini");
    }

    #[test]
    fn builds_chat_completions_url_from_base_url() {
        assert_eq!(
            chat_completions_url("http://localhost:1234/v1"),
            "http://localhost:1234/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://localhost:1234/v1/"),
            "http://localhost:1234/v1/chat/completions"
        );
    }

    #[test]
    fn builds_models_url_from_base_url() {
        assert_eq!(
            models_url("http://localhost:1234/v1"),
            "http://localhost:1234/v1/models"
        );
        assert_eq!(
            models_url("http://localhost:1234/v1/"),
            "http://localhost:1234/v1/models"
        );
    }

    #[test]
    fn resolves_default_custom_provider_route_from_local_settings() {
        let providers = vec![CustomProviderConfig {
            local_id: None,
            name: "local-openai".to_string(),
            base_url: "http://localhost:1234/v1".to_string(),
            models: vec!["qwen3-coder".to_string()],
            api_key_env_var: Some("LOCAL_OPENAI_API_KEY".to_string()),
            api_type: Default::default(),
            capabilities: Default::default(),
        }];
        let mut api_keys = ApiKeys::default();
        api_keys
            .custom
            .insert("local-openai".to_string(), "stored-key".to_string());

        let route = default_custom_provider_route(&providers, &api_keys).unwrap();

        assert_eq!(route.provider_name, "local-openai");
        assert_eq!(route.base_url, "http://localhost:1234/v1");
        assert_eq!(route.model, "qwen3-coder");
        assert_eq!(route.api_key.as_deref(), Some("stored-key"));
    }

    #[test]
    fn duplicate_custom_provider_names_fail_closed_without_route() {
        let providers = vec![
            CustomProviderConfig {
                local_id: Some("first".to_string()),
                name: "duplicate".to_string(),
                base_url: "http://localhost:1234/v1".to_string(),
                models: vec!["model".to_string()],
                ..Default::default()
            },
            CustomProviderConfig {
                local_id: Some("second".to_string()),
                name: "duplicate".to_string(),
                base_url: "http://localhost:5678/v1".to_string(),
                models: vec!["model".to_string()],
                ..Default::default()
            },
        ];

        assert_eq!(
            resolve_custom_provider_route(
                "custom/duplicate/model",
                &providers,
                &ApiKeys::default()
            ),
            None
        );
        assert_eq!(
            default_custom_provider_route(&providers, &ApiKeys::default()),
            None
        );
        assert_eq!(
            default_custom_provider_route_with_error(&providers, &ApiKeys::default(), true)
                .unwrap_err()
                .to_string(),
            "custom provider name `duplicate` is ambiguous; rename one provider before using it"
        );
        assert_eq!(
            resolve_custom_provider_route_with_error(
                "custom/duplicate/model",
                &providers,
                &ApiKeys::default(),
            )
            .unwrap_err()
            .to_string(),
            "custom provider name `duplicate` is ambiguous; rename one provider before using it"
        );
    }

    #[test]
    fn duplicate_custom_provider_name_blocks_default_route_for_other_providers() {
        let providers = vec![
            CustomProviderConfig {
                local_id: Some("first".to_string()),
                name: "duplicate".to_string(),
                base_url: "http://localhost:1234/v1".to_string(),
                models: vec!["first-model".to_string()],
                ..Default::default()
            },
            CustomProviderConfig {
                local_id: Some("second".to_string()),
                name: "duplicate".to_string(),
                base_url: "http://localhost:1234/v1".to_string(),
                models: vec!["second-model".to_string()],
                ..Default::default()
            },
            CustomProviderConfig {
                local_id: Some("unique".to_string()),
                name: "unique".to_string(),
                base_url: "http://localhost:1234/v1".to_string(),
                models: vec!["unique-model".to_string()],
                ..Default::default()
            },
        ];

        let error = default_custom_provider_route_with_error(&providers, &ApiKeys::default(), true)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "custom provider name `duplicate` is ambiguous; rename one provider before using it"
        );
        assert!(default_custom_provider_route(&providers, &ApiKeys::default()).is_none());
    }

    #[test]
    fn custom_provider_route_waits_for_secure_key_hydration() {
        let providers = vec![CustomProviderConfig {
            local_id: Some("local".to_string()),
            name: "local".to_string(),
            base_url: "http://localhost:1234/v1".to_string(),
            models: vec!["model".to_string()],
            ..Default::default()
        }];
        let mut api_keys = ApiKeys::default();
        api_keys
            .custom
            .insert("local".to_string(), "stored-key".to_string());

        let loading_error = resolve_custom_provider_route_with_readiness(
            "custom/local/model",
            &providers,
            &api_keys,
            false,
        )
        .unwrap_err();
        assert_eq!(
            loading_error.to_string(),
            "Local API keys are still loading; retry the request when secure storage is ready."
        );
        assert!(
            resolve_custom_provider_route_with_readiness(
                "custom/local/model",
                &providers,
                &api_keys,
                true,
            )
            .unwrap()
            .map(|route| route.api_key.as_deref() == Some("stored-key"))
            .unwrap_or(false)
        );
        assert!(default_custom_provider_route_when_ready(&providers, &api_keys, false,).is_none());
        assert_eq!(
            default_custom_provider_route_when_ready(&providers, &api_keys, true,)
                .and_then(|route| route.api_key),
            Some("stored-key".to_string())
        );
    }

    #[test]
    fn configured_capabilities_are_retained_but_unimplemented_adapters_stay_disabled() {
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: "http://localhost:1234/v1".to_string(),
            model: "model".to_string(),
            api_key: None,
            capabilities: CustomProviderCapabilities {
                chat: true,
                tools: true,
                vision: true,
                embeddings: true,
                transcription: true,
                context_window_tokens: Some(32_000),
            },
        };

        assert!(route.capabilities.vision);
        assert!(route.capabilities.embeddings);
        assert!(route.capabilities.transcription);
        assert_eq!(
            route.effective_capabilities(),
            EffectiveCustomProviderCapabilities {
                chat: true,
                tools: true,
                vision: false,
                embeddings: false,
                transcription: false,
            }
        );
    }

    #[test]
    fn configured_context_window_uses_conservative_character_budget() {
        let mut route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: "http://localhost:1234/v1".to_string(),
            model: "model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        assert_eq!(route.context_char_budget(), MAX_CONTEXT_CHARS);

        route.capabilities.context_window_tokens = Some(1_000);
        assert_eq!(route.context_char_budget(), 3_000);
    }

    #[tokio::test]
    async fn disabled_chat_returns_local_error_without_http_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .expect(0)
            .create_async()
            .await;
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: CustomProviderCapabilities {
                chat: false,
                ..Default::default()
            },
        };

        let mut response = generate(
            route,
            super::super::RequestParams::new_for_test(),
            vec![api::ToolType::RunShellCommand],
        )
        .await
        .expect("unsupported chat should degrade to a local response stream");
        let error = response
            .next()
            .await
            .expect("local error stream should produce one event")
            .expect_err("unsupported chat must be reported as a local error");

        assert!(error.to_string().contains("chat disabled"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn unimplemented_vision_returns_local_error_without_http_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .expect(0)
            .create_async()
            .await;
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: CustomProviderCapabilities {
                vision: true,
                ..Default::default()
            },
        };
        let mut params = super::super::RequestParams::new_for_test();
        params.input = vec![AIAgentInput::AutoCodeDiffQuery {
            query: "Describe this image".to_string(),
            context: std::sync::Arc::from(vec![AIAgentContext::Image(ImageContext {
                data: "redacted-test-image".to_string(),
                mime_type: "image/png".to_string(),
                file_name: "local.png".to_string(),
                is_figma: false,
            })]),
        }];

        let mut response = generate(route, params, vec![])
            .await
            .expect("unsupported vision should degrade to a local response stream");
        let error = response
            .next()
            .await
            .expect("local error stream should produce one event")
            .expect_err("unsupported vision must be reported as a local error");

        assert!(error.to_string().contains("vision is not implemented"));
        mock.assert_async().await;
    }

    #[test]
    fn disabled_tools_omit_openai_tool_request_fields() {
        let body = ChatCompletionRequest {
            model: "model".to_string(),
            messages: vec![ChatMessage::user("hello".to_string())],
            stream: true,
            tools: vec![],
            tool_choice: None,
            parallel_tool_calls: None,
        };
        let encoded = serde_json::to_value(body).expect("request should serialize");

        assert_eq!(encoded.get("model").and_then(Value::as_str), Some("model"));
        assert!(encoded.get("tools").is_none());
        assert!(encoded.get("tool_choice").is_none());
        assert!(encoded.get("parallel_tool_calls").is_none());
    }

    #[tokio::test]
    async fn disabled_tools_omit_fields_in_http_request() {
        let mut server = mockito::Server::new_async().await;
        let params = super::super::RequestParams::new_for_test();
        let expected_body = serde_json::to_value(ChatCompletionRequest {
            model: "model".to_string(),
            messages: openai_messages_from_params(&params, &[], MAX_CONTEXT_CHARS),
            stream: true,
            tools: vec![],
            tool_choice: None,
            parallel_tool_calls: None,
        })
        .expect("request should serialize");
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::Json(expected_body))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "choices": [
                        {
                            "message": { "content": "local answer" },
                            "finish_reason": "stop"
                        }
                    ]
                }"#,
            )
            .expect(1)
            .create_async()
            .await;
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: CustomProviderCapabilities {
                tools: false,
                ..Default::default()
            },
        };

        let mut response = generate(route, params, vec![api::ToolType::RunShellCommand])
            .await
            .expect("supported chat should produce a response stream");
        while let Some(event) = response.next().await {
            event.expect("mock response should decode");
        }

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn public_generate_non_streaming_tool_call_preserves_long_running_wait_gate() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "choices": [
                        {
                            "message": {
                                "tool_calls": [
                                    {
                                        "id": "call-run",
                                        "type": "function",
                                        "function": {
                                            "name": "run_shell_command",
                                            "arguments": "{\"command\":\"tail -f log\",\"wait_until_completion\":false}"
                                        }
                                    }
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }
                    ]
                }"#,
            )
            .create_async()
            .await;

        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        let mut response = generate(
            route,
            super::super::RequestParams::new_for_test(),
            vec![
                api::ToolType::RunShellCommand,
                api::ToolType::WriteToLongRunningShellCommand,
                api::ToolType::ReadShellCommandOutput,
                api::ToolType::TransferShellCommandControlToUser,
            ],
        )
        .await
        .expect("public generate should return a response stream");

        let mut run_shell_command = None;
        while let Some(event) = response.next().await {
            let event = event.expect("non-streaming tool call should decode");
            let Some(api::response_event::Type::ClientActions(actions)) = event.r#type else {
                continue;
            };
            for action in actions.actions {
                let Some(api::client_action::Action::AddMessagesToTask(add)) = action.action else {
                    continue;
                };
                for message in add.messages {
                    let Some(api::message::Message::ToolCall(tool_call)) = message.message else {
                        continue;
                    };
                    if let Some(api::message::tool_call::Tool::RunShellCommand(command)) =
                        tool_call.tool
                    {
                        run_shell_command = Some(command);
                    }
                }
            }
        }

        let run_shell_command = run_shell_command.expect("expected run_shell_command tool call");
        assert!(matches!(
            run_shell_command.wait_until_complete_value,
            Some(
                api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(
                    false
                )
            )
        ));
    }

    #[tokio::test]
    async fn public_generate_non_streaming_tool_call_rejects_disabled_tool_locally() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "choices": [
                        {
                            "message": {
                                "tool_calls": [
                                    {
                                        "id": "call-write",
                                        "type": "function",
                                        "function": {
                                            "name": "write_to_long_running_shell_command",
                                            "arguments": "{\"command_id\":\"block-1\",\"input\":\"yes\\n\"}"
                                        }
                                    }
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }
                    ]
                }"#,
            )
            .create_async()
            .await;

        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        let mut response = generate(
            route,
            super::super::RequestParams::new_for_test(),
            vec![api::ToolType::RunShellCommand],
        )
        .await
        .expect("public generate should return a response stream");

        let mut local_error = None;
        while let Some(event) = response.next().await {
            if let Err(error) = event {
                local_error = Some(error.to_string());
            }
        }

        let local_error = local_error.expect("disabled tool call should fail locally");
        assert!(local_error.contains("not advertised"));
    }

    #[test]
    fn openai_tool_call_envelope_rejects_non_function_or_empty_fields() {
        let call = |id: &str, kind: &str, name: &str, arguments: &str| OpenAIToolCall {
            id: id.to_string(),
            kind: kind.to_string(),
            function: OpenAIFunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        };

        for (label, tool_call) in [
            (
                "empty id",
                call("", "function", "run_shell_command", r#"{"command":"pwd"}"#),
            ),
            (
                "wrong type",
                call(
                    "call-1",
                    "message",
                    "run_shell_command",
                    r#"{"command":"pwd"}"#,
                ),
            ),
            (
                "empty function name",
                call("call-1", "function", "", r#"{"command":"pwd"}"#),
            ),
            (
                "non-object arguments",
                call("call-1", "function", "open_code_review", "[]"),
            ),
        ] {
            assert!(
                api_tool_from_openai_tool_call(&tool_call).is_err(),
                "{label} must fail closed"
            );
        }

        let valid = call(
            "call-1",
            "function",
            "run_shell_command",
            r#"{"command":"pwd"}"#,
        );
        assert!(api_tool_from_openai_tool_call(&valid).is_ok());
    }

    #[tokio::test]
    async fn public_generate_non_streaming_malformed_tool_envelopes_fail_locally() {
        for (label, id, kind, arguments) in [
            ("empty id", "", "function", r#"{"command":"pwd"}"#),
            ("wrong type", "call-1", "message", r#"{"command":"pwd"}"#),
            ("non-object arguments", "call-1", "function", "[]"),
        ] {
            let mut server = mockito::Server::new_async().await;
            let body = json!({
                "choices": [{
                    "message": {
                        "tool_calls": [{
                            "id": id,
                            "type": kind,
                            "function": {
                                "name": "run_shell_command",
                                "arguments": arguments
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            });
            let _mock = server
                .mock("POST", "/v1/chat/completions")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(body.to_string())
                .create_async()
                .await;
            let route = CustomProviderRoute {
                provider_name: "local".to_string(),
                base_url: format!("{}/v1", server.url()),
                model: "model".to_string(),
                api_key: None,
                capabilities: Default::default(),
            };
            let mut response = generate(
                route,
                super::super::RequestParams::new_for_test(),
                vec![api::ToolType::RunShellCommand],
            )
            .await
            .expect("public generate should return a response stream");
            let mut error = None;
            while let Some(event) = response.next().await {
                if let Err(error_value) = event {
                    error = Some(error_value.to_string());
                }
            }
            assert!(error.is_some(), "{label} must produce a local stream error");
        }
    }

    #[tokio::test]
    async fn public_generate_streamed_valid_tool_call_is_emitted() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"run_shell_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                    "data: [DONE]\n\n",
                )
            )
            .create_async()
            .await;
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        let mut response = generate(
            route,
            super::super::RequestParams::new_for_test(),
            vec![api::ToolType::RunShellCommand],
        )
        .await
        .expect("public generate should return a response stream");
        let mut saw_tool_call = false;
        while let Some(event) = response.next().await {
            let event = event.expect("valid SSE tool call should decode");
            let Some(api::response_event::Type::ClientActions(actions)) = event.r#type else {
                continue;
            };
            for action in actions.actions {
                let Some(api::client_action::Action::AddMessagesToTask(add)) = action.action else {
                    continue;
                };
                saw_tool_call |= add.messages.iter().any(|message| {
                    matches!(message.message, Some(api::message::Message::ToolCall(_)))
                });
            }
        }
        assert!(saw_tool_call);
    }

    #[tokio::test]
    async fn public_generate_round_trips_parallel_tool_calls_as_one_assistant_message() {
        let mut server = mockito::Server::new_async().await;
        let _first_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(concat!(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
                "{\"index\":0,\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"run_shell_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}},",
                "{\"index\":1,\"id\":\"call-2\",\"type\":\"function\",\"function\":{\"name\":\"grep\",\"arguments\":\"{\\\"queries\\\":[\\\"needle\\\"]}\"}}",
                "]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: [DONE]\n\n",
            ))
            .create_async()
            .await;
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        let mut first_response = generate(
            route.clone(),
            super::super::RequestParams::new_for_test(),
            vec![api::ToolType::RunShellCommand, api::ToolType::Grep],
        )
        .await
        .expect("first public request should return a stream");
        let mut tool_messages = Vec::new();
        while let Some(event) = first_response.next().await {
            let event = event.expect("parallel tool call response should decode");
            let Some(api::response_event::Type::ClientActions(actions)) = event.r#type else {
                continue;
            };
            for action in actions.actions {
                let Some(api::client_action::Action::AddMessagesToTask(add)) = action.action else {
                    continue;
                };
                tool_messages.extend(add.messages.into_iter().filter(|message| {
                    matches!(message.message, Some(api::message::Message::ToolCall(_)))
                }));
            }
        }
        assert_eq!(tool_messages.len(), 2);

        let mut next_params = super::super::RequestParams::new_for_test();
        next_params.tasks = vec![api::Task {
            id: "local-root-task".to_string(),
            messages: tool_messages,
            ..Default::default()
        }];
        let expected_messages = openai_messages_from_params_with_tool_policy(
            &next_params,
            &openai_tools_for_supported_tools(&[
                api::ToolType::RunShellCommand,
                api::ToolType::Grep,
            ]),
            MAX_CONTEXT_CHARS,
            false,
        );
        let _second_mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::Json(
                serde_json::to_value(ChatCompletionRequest {
                    model: route.model.clone(),
                    messages: expected_messages,
                    stream: true,
                    tools: openai_tools_for_supported_tools(&[
                        api::ToolType::RunShellCommand,
                        api::ToolType::Grep,
                    ]),
                    tool_choice: Some("auto"),
                    parallel_tool_calls: Some(true),
                })
                .unwrap(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#)
            .create_async()
            .await;
        let mut second_response = generate(
            route,
            next_params,
            vec![api::ToolType::RunShellCommand, api::ToolType::Grep],
        )
        .await
        .expect("second public request should return a stream");
        while let Some(event) = second_response.next().await {
            event.expect("second public request should decode");
        }
    }

    #[tokio::test]
    async fn public_generate_streamed_malformed_sse_fails_locally() {
        for body in [
            "data: {not-json}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"type\":\"message\",\"function\":{\"name\":\"run_shell_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call-2\",\"type\":\"function\",\"function\":{\"name\":\"run_shell_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        ] {
            let mut server = mockito::Server::new_async().await;
            let _mock = server
                .mock("POST", "/v1/chat/completions")
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body(body)
                .create_async()
                .await;
            let route = CustomProviderRoute {
                provider_name: "local".to_string(),
                base_url: format!("{}/v1", server.url()),
                model: "model".to_string(),
                api_key: None,
                capabilities: Default::default(),
            };
            let mut response = generate(
                route,
                super::super::RequestParams::new_for_test(),
                vec![api::ToolType::RunShellCommand],
            )
            .await
            .expect("public generate should return a response stream");
            let mut error = None;
            while let Some(event) = response.next().await {
                if let Err(error_value) = event {
                    error = Some(error_value.to_string());
                }
            }
            assert!(error.is_some(), "malformed SSE must fail locally");
        }
    }

    #[tokio::test]
    async fn configured_context_budget_bounds_http_message_context() {
        let mut server = mockito::Server::new_async().await;
        let mut params = super::super::RequestParams::new_for_test();
        params.input = vec![AIAgentInput::AutoCodeDiffQuery {
            query: "Review the selected text".to_string(),
            context: std::sync::Arc::from(vec![AIAgentContext::SelectedText("x".repeat(2_000))]),
        }];
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: CustomProviderCapabilities {
                context_window_tokens: Some(256),
                ..Default::default()
            },
        };
        let context_budget = route.context_char_budget();
        let tools = openai_tools_for_supported_tools(&[api::ToolType::RunShellCommand]);
        let expected_body = serde_json::to_value(ChatCompletionRequest {
            model: route.model.clone(),
            messages: openai_messages_from_params(&params, &tools, context_budget),
            stream: true,
            tools,
            tool_choice: Some("auto"),
            parallel_tool_calls: Some(true),
        })
        .expect("request should serialize");
        let expected_messages = expected_body
            .get("messages")
            .expect("request should include messages")
            .to_string();
        assert!(expected_messages.contains("[truncated]"));
        assert!(!expected_messages.contains(&"x".repeat(MAX_CONTEXT_CHARS)));

        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::Json(expected_body))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "choices": [
                        {
                            "message": { "content": "bounded" },
                            "finish_reason": "stop"
                        }
                    ]
                }"#,
            )
            .expect(1)
            .create_async()
            .await;

        let mut response = generate(route, params, vec![api::ToolType::RunShellCommand])
            .await
            .expect("configured context should produce a response stream");
        while let Some(event) = response.next().await {
            event.expect("mock response should decode");
        }

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn legacy_context_budget_preserves_http_fallback() {
        let mut server = mockito::Server::new_async().await;
        let mut params = super::super::RequestParams::new_for_test();
        params.input = vec![AIAgentInput::AutoCodeDiffQuery {
            query: "Review the selected text".to_string(),
            context: std::sync::Arc::from(vec![AIAgentContext::SelectedText(
                "x".repeat(MAX_CONTEXT_CHARS + 1_000),
            )]),
        }];
        let route = CustomProviderRoute {
            provider_name: "legacy".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        let expected_body = serde_json::to_value(ChatCompletionRequest {
            model: route.model.clone(),
            messages: openai_messages_from_params(&params, &[], MAX_CONTEXT_CHARS),
            stream: true,
            tools: vec![],
            tool_choice: None,
            parallel_tool_calls: None,
        })
        .expect("request should serialize");
        let expected_messages = expected_body
            .get("messages")
            .expect("request should include messages")
            .to_string();
        assert!(expected_messages.contains("[truncated]"));

        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::Json(expected_body))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "choices": [
                        {
                            "message": { "content": "legacy bounded" },
                            "finish_reason": "stop"
                        }
                    ]
                }"#,
            )
            .expect(1)
            .create_async()
            .await;

        let mut response = generate(route, params, vec![])
            .await
            .expect("legacy route should produce a response stream");
        while let Some(event) = response.next().await {
            event.expect("mock response should decode");
        }

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn completes_text_through_openai_compatible_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "choices": [
                        {
                            "message": { "content": "local answer" },
                            "finish_reason": "stop"
                        }
                    ]
                }"#,
            )
            .create_async()
            .await;

        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "test-model".to_string(),
            api_key: Some("test-key".to_string()),
            capabilities: Default::default(),
        };

        let content = complete_text(
            route,
            "Answer briefly.".to_string(),
            "Say hello.".to_string(),
        )
        .await
        .unwrap();

        assert_eq!(content, "local answer");
    }

    #[test]
    fn create_task_event_initializes_local_root_task() {
        let event = create_task_event("task-1");
        let Some(api::response_event::Type::ClientActions(actions)) = event.r#type else {
            panic!("expected client actions event");
        };
        let Some(api::client_action::Action::CreateTask(create)) =
            actions.actions.into_iter().next().and_then(|a| a.action)
        else {
            panic!("expected create task action");
        };
        let task = create.task.unwrap();

        assert_eq!(task.id, "task-1");
        assert!(task.dependencies.is_none());
        assert!(task.messages.is_empty());
    }

    #[test]
    fn supported_tools_are_advertised_to_openai() {
        let tools = openai_tools_for_supported_tools(&[
            api::ToolType::RunShellCommand,
            api::ToolType::ReadFiles,
            api::ToolType::CallMcpTool,
            api::ToolType::ReadSkill,
        ]);
        let names = tools
            .iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "run_shell_command",
                "read_files",
                "call_mcp_tool",
                "read_skill"
            ]
        );
    }

    #[test]
    fn advertises_all_local_non_orchestration_tools_without_orchestration_names() {
        let tools = openai_tools_for_supported_tools(&[
            api::ToolType::RunShellCommand,
            api::ToolType::SearchCodebase,
            api::ToolType::ReadFiles,
            api::ToolType::ApplyFileDiffs,
            api::ToolType::Grep,
            api::ToolType::FileGlob,
            api::ToolType::FileGlobV2,
            api::ToolType::ReadMcpResource,
            api::ToolType::CallMcpTool,
            api::ToolType::WriteToLongRunningShellCommand,
            api::ToolType::SuggestNewConversation,
            api::ToolType::OpenCodeReview,
            api::ToolType::InitProject,
            api::ToolType::ReadDocuments,
            api::ToolType::EditDocuments,
            api::ToolType::CreateDocuments,
            api::ToolType::ReadShellCommandOutput,
            api::ToolType::UseComputer,
            api::ToolType::InsertReviewComments,
            api::ToolType::ReadSkill,
            api::ToolType::RequestComputerUse,
            api::ToolType::FetchConversation,
            api::ToolType::TransferShellCommandControlToUser,
            api::ToolType::AskUserQuestion,
            api::ToolType::Subagent,
            api::ToolType::StartAgent,
            api::ToolType::StartAgentV2,
            api::ToolType::RunAgents,
            api::ToolType::SendMessageToAgent,
        ]);
        let names = tools
            .iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "run_shell_command",
                "read_files",
                "search_codebase",
                "grep",
                "file_glob",
                "apply_file_diffs",
                "read_mcp_resource",
                "call_mcp_tool",
                "read_skill",
                "write_to_long_running_shell_command",
                "read_shell_command_output",
                "transfer_shell_command_control_to_user",
                "suggest_new_conversation",
                "open_code_review",
                "init_project",
                "read_documents",
                "edit_documents",
                "create_documents",
                "insert_review_comments",
                "fetch_conversation",
                "ask_user_question",
                "use_computer",
                "request_computer_use",
            ]
        );
        assert!(names.iter().all(|name| {
            !matches!(
                *name,
                "subagent"
                    | "start_agent"
                    | "start_agent_v2"
                    | "run_agents"
                    | "send_message_to_agent"
            )
        }));
    }

    #[test]
    fn parses_local_tool_call_variants_and_validates_required_arguments() {
        let cases = [
            (
                "suggest_new_conversation",
                json!({"message_id":"message-1"}),
                "suggest_new_conversation",
            ),
            (
                "read_documents",
                json!({"documents":[{"document_id":"00000000-0000-0000-0000-000000000001","line_ranges":[{"start":2,"end":4}]}]}),
                "read_documents",
            ),
            (
                "edit_documents",
                json!({"diffs":[{"document_id":"00000000-0000-0000-0000-000000000001","search":"old","replace":"new"}]}),
                "edit_documents",
            ),
            (
                "create_documents",
                json!({"documents":[{"title":"Plan","content":"body"}]}),
                "create_documents",
            ),
            (
                "insert_review_comments",
                json!({"repo_path":"/repo","base_branch":"master","comments":[{"comment_id":"c-1","author":"reviewer","last_modified_timestamp":"2026-01-01T00:00:00Z","comment_body":"fix","parent_comment_id":"","html_url":"","location":{"file_path":"src/lib.rs","line":{"diff_hunk":"@@","range":{"start":3,"end":4},"side":"NEW"}}}]}),
                "insert_review_comments",
            ),
            (
                "fetch_conversation",
                json!({"conversation_id":"conversation-1"}),
                "fetch_conversation",
            ),
            (
                "ask_user_question",
                json!({"questions":[{"question_id":"q-1","question":"Choose","multiple_choice":{"options":[{"label":"yes"}],"recommended_option_index":0,"is_multiselect":false,"supports_other":true}}]}),
                "ask_user_question",
            ),
            (
                "request_computer_use",
                json!({"task_summary":"Inspect the screen","screenshot_params":{"max_long_edge_px":1000}}),
                "request_computer_use",
            ),
            (
                "use_computer",
                json!({"action_summary":"Click","actions":[{"type":"mouse_move","to":{"x":10,"y":20}},{"type":"type_text","text":"hello"}],"post_actions_screenshot_params":{}}),
                "use_computer",
            ),
            (
                "write_to_long_running_shell_command",
                json!({"command_id":"block-1","input":"echo\n","mode":"line"}),
                "write_to_long_running_shell_command",
            ),
            (
                "read_shell_command_output",
                json!({"command_id":"block-1","delay":{"kind":"duration","seconds":2}}),
                "read_shell_command_output",
            ),
            (
                "transfer_shell_command_control_to_user",
                json!({"reason":"The command needs interactive input."}),
                "transfer_shell_command_control_to_user",
            ),
        ];

        for (name, arguments, expected_name) in cases {
            let call = OpenAIToolCall {
                id: format!("call-{name}"),
                kind: "function".to_string(),
                function: OpenAIFunctionCall {
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                },
            };
            let parsed = api_tool_from_openai_tool_call(&call)
                .unwrap_or_else(|error| panic!("{name} should parse: {error}"));
            assert_eq!(
                openai_tool_name(&parsed),
                expected_name,
                "parsed protobuf variant for {name}"
            );
        }

        let missing_document_id = OpenAIToolCall {
            id: "call-invalid".to_string(),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "read_documents".to_string(),
                arguments: r#"{"documents":[{}]}"#.to_string(),
            },
        };
        assert!(api_tool_from_openai_tool_call(&missing_document_id).is_err());
    }

    #[test]
    fn rejects_invalid_optional_arguments_in_new_tool_parsers() {
        let call = |name: &str, arguments: Value| OpenAIToolCall {
            id: format!("call-{name}"),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        };
        let invalid_calls = [
            (
                "run_shell_command",
                json!({"command":"pwd","wait_until_completion":"false"}),
            ),
            (
                "write_to_long_running_shell_command",
                json!({"command_id":"block-1","input":"x","mode":1}),
            ),
            (
                "read_shell_command_output",
                json!({"command_id":"block-1","delay_seconds":"2"}),
            ),
            (
                "create_documents",
                json!({"documents":[{"content":"body","title":false}]}),
            ),
            (
                "insert_review_comments",
                json!({"repo_path":"/repo","comments":[{"comment_id":"c-1","comment_body":"fix","location":{"file_path":"src/lib.rs","line":{"side":1}}}]}),
            ),
            (
                "ask_user_question",
                json!({"questions":[{"question_id":"q-1","question":"Choose","multiple_choice":{"options":[{"label":"yes"}],"recommended_option_index":"0"}}]}),
            ),
            (
                "ask_user_question",
                json!({"questions":[{"question_id":"q-1","question":"Choose","multiple_choice":{"options":[{"label":"yes"}],"is_multiselect":"false"}}]}),
            ),
            (
                "ask_user_question",
                json!({"questions":[{"question_id":"q-1","question":"Choose","multiple_choice":{"options":[{"label":"yes"}],"supports_other":1}}]}),
            ),
            (
                "use_computer",
                json!({"actions":[{"type":"wait","seconds":1}],"post_actions_screenshot_params":"full-screen"}),
            ),
            (
                "request_computer_use",
                json!({"task_summary":"Inspect","screenshot_params":1}),
            ),
            (
                "request_computer_use",
                json!({"task_summary":"Inspect","screenshot_params":{"max_long_edge_px":"1000"}}),
            ),
        ];

        for (name, arguments) in invalid_calls {
            assert!(
                api_tool_from_openai_tool_call(&call(name, arguments)).is_err(),
                "{name} should reject an optional field with the wrong JSON type"
            );
        }
    }

    #[test]
    fn legacy_direct_tool_parsers_reject_present_wrong_types_and_ranges() {
        let call = |name: &str, arguments: Value| OpenAIToolCall {
            id: format!("call-{name}"),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        };
        let invalid_calls = [
            ("read_files", json!({"files": {}})),
            (
                "read_files",
                json!({"files":[{"name":"file","line_ranges":"all"}]}),
            ),
            (
                "read_files",
                json!({"files":[{"name":"file","line_ranges":[{"start":"1","end":2}]}]}),
            ),
            ("search_codebase", json!({"query":"x","path_filters":[1]})),
            (
                "search_codebase",
                json!({"query":"x","codebase_path":false}),
            ),
            ("grep", json!({"queries": {"query":"x"}})),
            ("grep", json!({"query":"x","path":false})),
            ("file_glob", json!({"patterns":[1]})),
            ("file_glob", json!({"pattern":"*.rs","max_matches":"10"})),
            ("file_glob", json!({"pattern":"*.rs","max_depth":-1})),
            (
                "read_mcp_resource",
                json!({"uri":"file://repo","server_id":1}),
            ),
            (
                "call_mcp_tool",
                json!({"name":"read","args":{},"server_id":1}),
            ),
            ("read_skill", json!({"skill_path":false})),
            (
                "read_skill",
                json!({"skill_path":"skills/example","name":1}),
            ),
            ("apply_file_diffs", json!({"summary":1})),
            ("apply_file_diffs", json!({"diffs":{}})),
            (
                "apply_file_diffs",
                json!({"diffs":[{"file_path":"file","search":"old","replace":"new"}],"new_files":"none"}),
            ),
            (
                "apply_file_diffs",
                json!({"diffs":[{"file_path":"file","search":"old","replace":"new"}],"deleted_files":false}),
            ),
        ];

        for (name, arguments) in invalid_calls {
            assert!(
                api_tool_from_openai_tool_call(&call(name, arguments)).is_err(),
                "{name} must reject a present field with the wrong type or range"
            );
        }
    }

    #[test]
    fn computer_wait_and_screenshot_regions_require_complete_semantics() {
        let call = |name: &str, arguments: Value| OpenAIToolCall {
            id: format!("call-{name}"),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        };

        for arguments in [
            json!({"actions":[{"type":"wait"}]}),
            json!({"actions":[{"type":"wait","seconds":null}]}),
            json!({"actions":[{"type":"mouse_move","to":{"x":1,"y":2}}],"post_actions_screenshot_params":{"region":{"top_left":{"x":0,"y":0}}}}),
            json!({"task_summary":"inspect","screenshot_params":{"region":{"bottom_right":{"x":10,"y":10}}}}),
        ] {
            let name = if arguments.get("actions").is_some() {
                "use_computer"
            } else {
                "request_computer_use"
            };
            assert!(
                api_tool_from_openai_tool_call(&call(name, arguments)).is_err(),
                "{name} must reject incomplete computer-use semantics"
            );
        }
    }

    #[test]
    fn history_groups_contiguous_parallel_tool_calls_and_preserves_results() {
        let first = api::Message {
            id: "call-message-1".to_string(),
            task_id: "task-1".to_string(),
            request_id: "request-1".to_string(),
            timestamp: None,
            server_message_data: String::new(),
            citations: vec![],
            message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                tool_call_id: "call-1".to_string(),
                tool: Some(api::message::tool_call::Tool::RunShellCommand(
                    api::message::tool_call::RunShellCommand {
                        command: "pwd".to_string(),
                        ..Default::default()
                    },
                )),
            })),
        };
        let second = api::Message {
            id: "call-message-2".to_string(),
            task_id: "task-1".to_string(),
            request_id: "request-1".to_string(),
            timestamp: None,
            server_message_data: String::new(),
            citations: vec![],
            message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                tool_call_id: "call-2".to_string(),
                tool: Some(api::message::tool_call::Tool::Grep(
                    api::message::tool_call::Grep {
                        queries: vec!["needle".to_string()],
                        path: "src".to_string(),
                    },
                )),
            })),
        };
        let result = api::Message {
            id: "result-message".to_string(),
            task_id: "task-1".to_string(),
            request_id: "request-2".to_string(),
            timestamp: None,
            server_message_data: String::new(),
            citations: vec![],
            message: Some(api::message::Message::ToolCallResult(
                api::message::ToolCallResult {
                    tool_call_id: "call-1".to_string(),
                    context: None,
                    result: Some(api::message::tool_call_result::Result::Grep(
                        api::GrepResult {
                            result: Some(api::grep_result::Result::Error(
                                api::grep_result::Error {
                                    message: "no match".to_string(),
                                },
                            )),
                        },
                    )),
                },
            )),
        };
        let params = super::super::RequestParams {
            tasks: vec![api::Task {
                id: "task-1".to_string(),
                messages: vec![first, second, result],
                ..Default::default()
            }],
            ..super::super::RequestParams::new_for_test()
        };

        let messages =
            openai_messages_from_params_with_tool_policy(&params, &[], MAX_CONTEXT_CHARS, false);
        let assistant = messages
            .iter()
            .find(|message| message.role == "assistant")
            .expect("parallel calls should produce an assistant message");
        assert_eq!(assistant.tool_calls.len(), 2);
        assert_eq!(assistant.tool_calls[0].id, "call-1");
        assert_eq!(assistant.tool_calls[1].id, "call-2");
        let tool = messages
            .iter()
            .find(|message| message.role == "tool")
            .expect("tool result should remain a separate tool message");
        assert_eq!(tool.tool_call_id.as_deref(), Some("call-1"));
        let content: Value = serde_json::from_str(tool.content.as_deref().unwrap()).unwrap();
        assert_eq!(content["version"], 1);
        assert_eq!(content["tool_call_id"], "call-1");
    }

    #[test]
    fn tool_results_use_versioned_structured_json_and_round_trip_protobuf() {
        let result = api::message::ToolCallResult {
            tool_call_id: "call-1".to_string(),
            context: None,
            result: Some(
                api::message::tool_call_result::Result::ReadShellCommandOutput(
                    api::ReadShellCommandOutputResult {
                        command: "tail -f log".to_string(),
                        result: Some(api::read_shell_command_output_result::Result::Error(
                            api::ShellCommandError {
                                r#type: Some(api::shell_command_error::Type::CommandNotFound(())),
                            },
                        )),
                    },
                ),
            ),
        };
        let content = tool_call_result_to_text_with_tool_policy(&result, true);
        let envelope: Value = serde_json::from_str(&content).expect("tool content must be JSON");
        assert_eq!(envelope["schema"], "warp.direct_openai.tool_result");
        assert_eq!(envelope["version"], 1);
        assert_eq!(envelope["tool_call_id"], "call-1");
        assert_eq!(envelope["result_type"], "read_shell_command_output");
        let encoded = envelope["protobuf_base64"].as_str().unwrap();
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        use prost::Message as _;
        let round_trip = api::message::ToolCallResult::decode(bytes.as_slice()).unwrap();
        assert_eq!(round_trip, result);
    }

    #[test]
    fn shell_delay_seconds_and_nanos_survive_direct_action_conversion() {
        let action = ai::agent::action::AIAgentActionType::from(
            api::message::tool_call::ReadShellCommandOutput {
                command_id: "block-1".to_string(),
                delay: Some(
                    api::message::tool_call::read_shell_command_output::Delay::Duration(
                        prost_types::Duration {
                            seconds: 2,
                            nanos: 345_678_901,
                        },
                    ),
                ),
            },
        );
        let ai::agent::action::AIAgentActionType::ReadShellCommandOutput { delay, .. } = action
        else {
            panic!("expected read shell command output action");
        };
        assert_eq!(
            delay,
            Some(ai::agent::action::ShellCommandDelay::Duration(
                std::time::Duration::new(2, 345_678_901),
            ))
        );
    }

    #[test]
    fn history_tool_arguments_round_trip_new_local_tool_semantics() {
        let document_id = "00000000-0000-0000-0000-000000000001".to_string();
        let tools = vec![
            api::message::tool_call::Tool::SuggestNewConversation(
                api::message::tool_call::SuggestNewConversation {
                    message_id: "message-1".to_string(),
                },
            ),
            api::message::tool_call::Tool::OpenCodeReview(
                api::message::tool_call::OpenCodeReview {},
            ),
            api::message::tool_call::Tool::InitProject(api::message::tool_call::InitProject {}),
            api::message::tool_call::Tool::ReadDocuments(
                api::message::tool_call::ReadDocuments {
                    documents: vec![api::message::tool_call::read_documents::Document {
                        document_id: document_id.clone(),
                        line_ranges: vec![api::FileContentLineRange { start: 2, end: 4 }],
                    }],
                },
            ),
            api::message::tool_call::Tool::EditDocuments(
                api::message::tool_call::EditDocuments {
                    diffs: vec![api::message::tool_call::edit_documents::DocumentDiff {
                        document_id: document_id.clone(),
                        search: "old".to_string(),
                        replace: "new".to_string(),
                    }],
                },
            ),
            api::message::tool_call::Tool::CreateDocuments(
                api::message::tool_call::CreateDocuments {
                    new_documents: vec![
                        api::message::tool_call::create_documents::NewDocument {
                            content: "body".to_string(),
                            title: "Plan".to_string(),
                        },
                    ],
                },
            ),
            api::message::tool_call::Tool::InsertReviewComments(
                api::message::tool_call::InsertReviewComments {
                    repo_path: "/repo".to_string(),
                    base_branch: "master".to_string(),
                    comments: vec![
                        api::message::tool_call::insert_review_comments::Comment {
                            comment_id: "c-1".to_string(),
                            comment_body: "fix".to_string(),
                            location: Some(
                                api::message::tool_call::insert_review_comments::CommentLocation {
                                    file_path: "src/lib.rs".to_string(),
                                    line: Some(
                                        api::message::tool_call::insert_review_comments::CommentLineRange {
                                            diff_hunk: "@@".to_string(),
                                            range: None,
                                            side: api::message::tool_call::insert_review_comments::CommentSide::Old as i32,
                                        },
                                    ),
                                },
                            ),
                            ..Default::default()
                        },
                        api::message::tool_call::insert_review_comments::Comment {
                            comment_id: "c-2".to_string(),
                            comment_body: "general note".to_string(),
                            location: None,
                            ..Default::default()
                        },
                    ],
                },
            ),
            api::message::tool_call::Tool::FetchConversation(
                api::message::tool_call::FetchConversation {
                    conversation_id: "conversation-1".to_string(),
                },
            ),
            api::message::tool_call::Tool::AskUserQuestion(api::AskUserQuestion {
                questions: vec![api::ask_user_question::Question {
                    question_id: "q-1".to_string(),
                    question: "Choose".to_string(),
                    question_type: Some(
                        api::ask_user_question::question::QuestionType::MultipleChoice(
                            api::ask_user_question::MultipleChoice {
                                options: vec![
                                    api::ask_user_question::Option {
                                        label: "yes".to_string(),
                                    },
                                    api::ask_user_question::Option {
                                        label: "no".to_string(),
                                    },
                                ],
                                recommended_option_index: 1,
                                is_multiselect: true,
                                supports_other: true,
                            },
                        ),
                    ),
                }],
            }),
            api::message::tool_call::Tool::AskUserQuestion(api::AskUserQuestion {
                questions: vec![api::ask_user_question::Question {
                    question_id: "q-2".to_string(),
                    question: "No type".to_string(),
                    question_type: None,
                }],
            }),
            api::message::tool_call::Tool::UseComputer(
                api::message::tool_call::UseComputer {
                    actions: vec![
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::MouseMove(
                                    api::message::tool_call::use_computer::action::MouseMove {
                                        to: Some(api::Coordinates { x: 10, y: 20 }),
                                    },
                                ),
                            ),
                        },
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::MouseDown(
                                    api::message::tool_call::use_computer::action::MouseDown {
                                        button: api::message::tool_call::use_computer::action::MouseButton::Left as i32,
                                        at: Some(api::Coordinates { x: 10, y: 20 }),
                                    },
                                ),
                            ),
                        },
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::MouseUp(
                                    api::message::tool_call::use_computer::action::MouseUp {
                                        button: api::message::tool_call::use_computer::action::MouseButton::Right as i32,
                                    },
                                ),
                            ),
                        },
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::MouseWheel(
                                    api::message::tool_call::use_computer::action::MouseWheel {
                                        at: Some(api::Coordinates { x: 10, y: 20 }),
                                        direction: api::message::tool_call::use_computer::action::mouse_wheel::Direction::Down as i32,
                                        distance: Some(
                                            api::message::tool_call::use_computer::action::mouse_wheel::Distance::Pixels(-4),
                                        ),
                                    },
                                ),
                            ),
                        },
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::Wait(
                                    api::message::tool_call::use_computer::action::Wait {
                                        duration: Some(prost_types::Duration {
                                            seconds: 2,
                                            nanos: 123,
                                        }),
                                    },
                                ),
                            ),
                        },
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::TypeText(
                                    api::message::tool_call::use_computer::action::TypeText {
                                        text: "hello".to_string(),
                                    },
                                ),
                            ),
                        },
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::KeyDown(
                                    api::message::tool_call::use_computer::action::KeyDown {
                                        key: Some(
                                            api::message::tool_call::use_computer::action::Key {
                                                data: Some(
                                                    api::message::tool_call::use_computer::action::key::Data::Char(
                                                        "x".to_string(),
                                                    ),
                                                ),
                                            },
                                        ),
                                    },
                                ),
                            ),
                        },
                        api::message::tool_call::use_computer::Action {
                            r#type: Some(
                                api::message::tool_call::use_computer::action::Type::KeyUp(
                                    api::message::tool_call::use_computer::action::KeyUp {
                                        key: Some(
                                            api::message::tool_call::use_computer::action::Key {
                                                data: Some(
                                                    api::message::tool_call::use_computer::action::key::Data::Keycode(
                                                        13,
                                                    ),
                                                ),
                                            },
                                        ),
                                    },
                                ),
                            ),
                        },
                    ],
                    post_actions_screenshot_params: Some(
                        api::message::tool_call::ScreenshotParams {
                            max_long_edge_px: 1000,
                            max_total_px: 2000,
                            region: Some(
                                api::message::tool_call::screenshot_params::Region {
                                    top_left: Some(api::Coordinates { x: 0, y: 0 }),
                                    bottom_right: Some(api::Coordinates { x: 640, y: 480 }),
                                },
                            ),
                        },
                    ),
                    action_summary: "Wait".to_string(),
                },
            ),
            api::message::tool_call::Tool::UseComputer(
                api::message::tool_call::UseComputer {
                    actions: vec![api::message::tool_call::use_computer::Action {
                        r#type: Some(
                            api::message::tool_call::use_computer::action::Type::Wait(
                                api::message::tool_call::use_computer::action::Wait {
                                    duration: Some(prost_types::Duration {
                                        seconds: 1,
                                        nanos: 234,
                                    }),
                                },
                            ),
                        ),
                    }],
                    ..Default::default()
                },
            ),
            api::message::tool_call::Tool::RunShellCommand(
                api::message::tool_call::RunShellCommand {
                    command: "tail -f log".to_string(),
                    wait_until_complete_value: Some(
                        api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(
                            false,
                        ),
                    ),
                    ..Default::default()
                },
            ),
            api::message::tool_call::Tool::RequestComputerUse(
                api::message::tool_call::RequestComputerUse {
                    task_summary: "Inspect".to_string(),
                    screenshot_params: Some(api::message::tool_call::ScreenshotParams {
                        max_long_edge_px: 800,
                        max_total_px: 1600,
                        region: None,
                    }),
                },
            ),
            api::message::tool_call::Tool::RequestComputerUse(
                api::message::tool_call::RequestComputerUse {
                    task_summary: "Inspect without screenshot".to_string(),
                    screenshot_params: None,
                },
            ),
            api::message::tool_call::Tool::WriteToLongRunningShellCommand(
                api::message::tool_call::WriteToLongRunningShellCommand {
                    command_id: "block-1".to_string(),
                    input: vec![0, 255, 10],
                    mode: Some(
                        api::message::tool_call::write_to_long_running_shell_command::Mode {
                            mode: Some(
                                api::message::tool_call::write_to_long_running_shell_command::mode::Mode::Block(
                                    (),
                                ),
                            ),
                        },
                    ),
                },
            ),
            api::message::tool_call::Tool::WriteToLongRunningShellCommand(
                api::message::tool_call::WriteToLongRunningShellCommand {
                    command_id: "block-2".to_string(),
                    input: b"raw".to_vec(),
                    mode: None,
                },
            ),
            api::message::tool_call::Tool::ReadShellCommandOutput(
                api::message::tool_call::ReadShellCommandOutput {
                    command_id: "block-1".to_string(),
                    delay: Some(
                        api::message::tool_call::read_shell_command_output::Delay::Duration(
                            prost_types::Duration {
                                seconds: 3,
                                nanos: 456,
                            },
                        ),
                    ),
                },
            ),
            api::message::tool_call::Tool::ReadShellCommandOutput(
                api::message::tool_call::ReadShellCommandOutput {
                    command_id: "block-2".to_string(),
                    delay: None,
                },
            ),
            api::message::tool_call::Tool::TransferShellCommandControlToUser(
                api::message::tool_call::TransferShellCommandControlToUser {
                    reason: "Needs input".to_string(),
                },
            ),
        ];

        for (index, expected) in tools.into_iter().enumerate() {
            let call = api::message::ToolCall {
                tool_call_id: format!("call-{index}"),
                tool: Some(expected.clone()),
            };
            let openai_call = openai_tool_call_from_api_tool_call(&call)
                .unwrap_or_else(|| panic!("tool {index} should serialize"));
            let parsed = api_tool_from_openai_tool_call(&openai_call)
                .unwrap_or_else(|error| panic!("tool {index} should parse: {error}"));
            assert_eq!(parsed, expected, "tool {index} semantic round-trip");
        }
    }

    #[test]
    fn local_tool_result_history_preserves_long_running_status_and_error_variants() {
        let finished = api::message::ToolCallResult {
            tool_call_id: "call-finished".to_string(),
            context: None,
            result: Some(
                api::message::tool_call_result::Result::ReadShellCommandOutput(
                    api::ReadShellCommandOutputResult {
                        command: "cat".to_string(),
                        result: Some(
                            api::read_shell_command_output_result::Result::CommandFinished(
                                api::ShellCommandFinished {
                                    output: "done".to_string(),
                                    exit_code: 0,
                                    command_id: "block-1".to_string(),
                                    ..Default::default()
                                },
                            ),
                        ),
                    },
                ),
            ),
        };
        let error = api::message::ToolCallResult {
            tool_call_id: "call-error".to_string(),
            context: None,
            result: Some(
                api::message::tool_call_result::Result::ReadShellCommandOutput(
                    api::ReadShellCommandOutputResult {
                        command: "cat".to_string(),
                        result: Some(api::read_shell_command_output_result::Result::Error(
                            api::ShellCommandError {
                                r#type: Some(api::shell_command_error::Type::CommandNotFound(())),
                            },
                        )),
                    },
                ),
            ),
        };

        let finished_text = tool_call_result_to_text_with_tool_policy(&finished, true);
        let error_text = tool_call_result_to_text_with_tool_policy(&error, true);
        for (text, expected) in [(&finished_text, &finished), (&error_text, &error)] {
            let envelope: Value = serde_json::from_str(text)
                .expect("structured tool result should remain valid JSON");
            assert_eq!(envelope["schema"], "warp.direct_openai.tool_result");
            assert_eq!(envelope["version"], 1);
            let encoded = envelope["protobuf_base64"]
                .as_str()
                .expect("complete result should include protobuf payload");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("structured tool result payload should be base64");
            let decoded = api::message::ToolCallResult::decode(bytes.as_slice())
                .expect("structured tool result payload should decode");
            assert_eq!(&decoded, expected);
        }
    }

    #[test]
    fn computer_use_tools_publish_exact_discriminated_action_schemas() {
        let tools = openai_tools_for_supported_tools(&[
            api::ToolType::UseComputer,
            api::ToolType::RequestComputerUse,
        ]);
        let use_computer = tools
            .iter()
            .find(|tool| tool.function.name == "use_computer")
            .expect("use_computer schema");
        let actions = &use_computer.function.parameters["properties"]["actions"]["items"];
        let variants = actions["oneOf"]
            .as_array()
            .expect("computer actions should use oneOf");
        assert_eq!(variants.len(), 8);
        for variant in variants {
            assert_eq!(variant["type"], "object");
            assert_eq!(variant["additionalProperties"], false);
            assert!(
                variant["required"]
                    .as_array()
                    .is_some_and(|required| required.iter().any(|value| value == "type"))
            );
        }
        let mouse_move = variants
            .iter()
            .find(|variant| variant["properties"]["type"]["const"] == "mouse_move")
            .expect("mouse_move variant");
        assert_eq!(mouse_move["required"], json!(["type", "to"]));
        assert_eq!(
            mouse_move["properties"]["to"],
            json!({
                "type": "object",
                "properties": {
                    "x": {"type":"integer","minimum":-2147483648_i64,"maximum":2147483647_i64},
                    "y": {"type":"integer","minimum":-2147483648_i64,"maximum":2147483647_i64}
                },
                "required": ["x", "y"],
                "additionalProperties": false
            })
        );

        let request = tools
            .iter()
            .find(|tool| tool.function.name == "request_computer_use")
            .expect("request_computer_use schema");
        let screenshot = &request.function.parameters["properties"]["screenshot_params"];
        assert_eq!(screenshot["type"], "object");
        assert_eq!(screenshot["additionalProperties"], false);
        assert_eq!(
            screenshot["properties"]["max_long_edge_px"],
            json!({"type":"integer","minimum":0,"maximum":2147483647_i64})
        );
        assert_eq!(
            screenshot["properties"]["max_total_px"],
            json!({"type":"integer","minimum":0,"maximum":2147483647_i64})
        );
        let region = &screenshot["properties"]["region"];
        let region_variants = region["oneOf"]
            .as_array()
            .expect("region should allow null or a complete object");
        assert_eq!(region_variants[0], json!({"type": "null"}));
        let region_object = &region_variants[1];
        assert_eq!(region_object["type"], "object");
        assert_eq!(
            region_object["required"],
            json!(["top_left", "bottom_right"])
        );
        assert_eq!(region_object["additionalProperties"], false);
        for coordinate in ["top_left", "bottom_right"] {
            assert_eq!(
                region_object["properties"][coordinate],
                json!({
                    "type":"object",
                    "properties": {
                        "x": {"type":"integer","minimum":-2147483648_i64,"maximum":2147483647_i64},
                        "y": {"type":"integer","minimum":-2147483648_i64,"maximum":2147483647_i64}
                    },
                    "required": ["x", "y"],
                    "additionalProperties": false
                })
            );
        }
    }

    #[test]
    fn history_serializes_every_new_local_tool_with_same_function_name() {
        let calls = vec![
            api::message::ToolCall {
                tool_call_id: "suggest".to_string(),
                tool: Some(api::message::tool_call::Tool::SuggestNewConversation(
                    api::message::tool_call::SuggestNewConversation {
                        message_id: "message-1".to_string(),
                    },
                )),
            },
            api::message::ToolCall {
                tool_call_id: "read-docs".to_string(),
                tool: Some(api::message::tool_call::Tool::ReadDocuments(
                    api::message::tool_call::ReadDocuments {
                        documents: vec![api::message::tool_call::read_documents::Document {
                            document_id: "00000000-0000-0000-0000-000000000001".to_string(),
                            line_ranges: vec![],
                        }],
                    },
                )),
            },
            api::message::ToolCall {
                tool_call_id: "edit-docs".to_string(),
                tool: Some(api::message::tool_call::Tool::EditDocuments(
                    api::message::tool_call::EditDocuments {
                        diffs: vec![api::message::tool_call::edit_documents::DocumentDiff {
                            document_id: "00000000-0000-0000-0000-000000000001".to_string(),
                            search: "old".to_string(),
                            replace: "new".to_string(),
                        }],
                    },
                )),
            },
            api::message::ToolCall {
                tool_call_id: "create-docs".to_string(),
                tool: Some(api::message::tool_call::Tool::CreateDocuments(
                    api::message::tool_call::CreateDocuments {
                        new_documents: vec![
                            api::message::tool_call::create_documents::NewDocument {
                                content: "body".to_string(),
                                title: "Plan".to_string(),
                            },
                        ],
                    },
                )),
            },
            api::message::ToolCall {
                tool_call_id: "review".to_string(),
                tool: Some(api::message::tool_call::Tool::OpenCodeReview(
                    api::message::tool_call::OpenCodeReview {},
                )),
            },
            api::message::ToolCall {
                tool_call_id: "init".to_string(),
                tool: Some(api::message::tool_call::Tool::InitProject(
                    api::message::tool_call::InitProject {},
                )),
            },
            api::message::ToolCall {
                tool_call_id: "fetch".to_string(),
                tool: Some(api::message::tool_call::Tool::FetchConversation(
                    api::message::tool_call::FetchConversation {
                        conversation_id: "conversation-1".to_string(),
                    },
                )),
            },
        ];

        let names = calls
            .iter()
            .map(|call| {
                openai_tool_call_from_api_tool_call(call)
                    .unwrap()
                    .function
                    .name
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "suggest_new_conversation",
                "read_documents",
                "edit_documents",
                "create_documents",
                "open_code_review",
                "init_project",
                "fetch_conversation",
            ]
        );
    }

    #[test]
    fn run_shell_command_wait_is_preserved_only_with_complete_long_running_family() {
        let call = OpenAIToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "run_shell_command".to_string(),
                arguments: r#"{"command":"tail -f log","wait_until_completion":false}"#.to_string(),
            },
        };

        let without_controls = api_tool_from_openai_tool_call_with_supported_tools(
            &call,
            &[api::ToolType::RunShellCommand],
        )
        .unwrap();
        assert!(run_shell_wait_value(&without_controls));

        let with_controls = api_tool_from_openai_tool_call_with_supported_tools(
            &call,
            &[
                api::ToolType::RunShellCommand,
                api::ToolType::WriteToLongRunningShellCommand,
                api::ToolType::ReadShellCommandOutput,
                api::ToolType::TransferShellCommandControlToUser,
            ],
        )
        .unwrap();
        assert!(!run_shell_wait_value(&with_controls));
    }

    #[test]
    fn unsupported_and_unknown_openai_calls_fail_locally() {
        let known_but_disabled = OpenAIToolCall {
            id: "call-disabled".to_string(),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "ask_user_question".to_string(),
                arguments: json!({"questions": []}).to_string(),
            },
        };
        let error = api_tool_from_openai_tool_call_with_supported_tools(
            &known_but_disabled,
            &[api::ToolType::RunShellCommand],
        )
        .unwrap_err();
        assert!(error.to_string().contains("not advertised"));

        let unknown = OpenAIToolCall {
            id: "call-unknown".to_string(),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "warp_cloud_magic".to_string(),
                arguments: "{}".to_string(),
            },
        };
        assert!(api_tool_from_openai_tool_call(&unknown).is_err());
    }

    #[test]
    fn long_running_snapshots_are_not_sent_without_polling_tools() {
        let result = api::message::ToolCallResult {
            tool_call_id: "call-1".to_string(),
            context: None,
            result: Some(api::message::tool_call_result::Result::RunShellCommand(
                api::RunShellCommandResult {
                    command: "tail -f log".to_string(),
                    result: Some(
                        api::run_shell_command_result::Result::LongRunningCommandSnapshot(
                            api::LongRunningShellCommandSnapshot {
                                command_id: "block-1".to_string(),
                                ..Default::default()
                            },
                        ),
                    ),
                    ..Default::default()
                },
            )),
        };

        let safe_text = tool_call_result_to_text_with_tool_policy(&result, false);
        assert!(safe_text.contains("polling and control tools are not advertised"));

        let complete_text = tool_call_result_to_text_with_tool_policy(&result, true);
        let complete: Value = serde_json::from_str(&complete_text)
            .expect("structured tool result should remain valid JSON");
        assert_eq!(complete["schema"], "warp.direct_openai.tool_result");
        assert_eq!(complete["version"], 1);
        assert_eq!(complete["result_type"], "run_shell_command");
        assert!(complete["protobuf_base64"].as_str().is_some());
    }

    #[test]
    fn converts_openai_run_shell_command_tool_call_to_warp_message() {
        let call = OpenAIToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "run_shell_command".to_string(),
                arguments: r#"{"command":"pwd","wait_until_completion":true}"#.to_string(),
            },
        };

        let message = api_tool_call_message("task-1", "request-1", call).unwrap();
        let Some(api::message::Message::ToolCall(tool_call)) = message.message else {
            panic!("expected tool call message");
        };
        let Some(api::message::tool_call::Tool::RunShellCommand(command)) = tool_call.tool else {
            panic!("expected run shell command");
        };

        assert_eq!(tool_call.tool_call_id, "call-1");
        assert_eq!(command.command, "pwd");
        assert!(command.is_read_only);
    }

    #[test]
    fn coerces_openai_run_shell_command_to_wait_until_completion() {
        let call = OpenAIToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "run_shell_command".to_string(),
                arguments: r#"{"command":"du -sh target","wait_until_completion":false}"#
                    .to_string(),
            },
        };

        let message = api_tool_call_message("task-1", "request-1", call).unwrap();
        let Some(api::message::Message::ToolCall(tool_call)) = message.message else {
            panic!("expected tool call message");
        };
        let Some(api::message::tool_call::Tool::RunShellCommand(command)) = tool_call.tool else {
            panic!("expected run shell command");
        };

        assert!(matches!(
            command.wait_until_complete_value,
            Some(api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(true))
        ));
    }

    #[test]
    fn converts_openai_mcp_tool_call_to_warp_message() {
        let call = OpenAIToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "call_mcp_tool".to_string(),
                arguments:
                    r#"{"name":"read_repo","server_id":"srv-1","args":{"path":"/tmp/repo"}}"#
                        .to_string(),
            },
        };

        let message = api_tool_call_message("task-1", "request-1", call).unwrap();
        let Some(api::message::Message::ToolCall(tool_call)) = message.message else {
            panic!("expected tool call message");
        };
        let Some(api::message::tool_call::Tool::CallMcpTool(call)) = tool_call.tool else {
            panic!("expected MCP tool call");
        };

        assert_eq!(call.name, "read_repo");
        assert_eq!(call.server_id, "srv-1");
        assert_eq!(
            call.args
                .unwrap()
                .fields
                .get("path")
                .and_then(|value| value.kind.as_ref()),
            Some(&prost_types::value::Kind::StringValue(
                "/tmp/repo".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn streams_openai_content_as_progressive_client_actions() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(
                r#"data: {"choices":[{"delta":{"content":"hel"}}]}

data: {"choices":[{"delta":{"content":"lo"}}]}

data: [DONE]

"#,
            )
            .create_async()
            .await;

        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "test-model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        let body = ChatCompletionRequest {
            model: route.model.clone(),
            messages: vec![ChatMessage::user("hello".to_string())],
            stream: true,
            tools: vec![],
            tool_choice: None,
            parallel_tool_calls: None,
        };

        let mut stream = stream_chat_completion(
            route,
            body,
            "task-1".to_string(),
            "request-1".to_string(),
            vec![create_task_action("task-1")],
        );

        let first = stream.next().await.unwrap().unwrap();
        let Some(api::response_event::Type::ClientActions(actions)) = first.r#type else {
            panic!("expected client actions");
        };
        assert_eq!(actions.actions.len(), 2);
        let Some(api::client_action::Action::AddMessagesToTask(add)) = actions
            .actions
            .into_iter()
            .nth(1)
            .and_then(|action| action.action)
        else {
            panic!("expected add messages action");
        };
        let Some(api::message::Message::AgentOutput(output)) =
            add.messages.into_iter().next().unwrap().message
        else {
            panic!("expected agent output");
        };
        assert_eq!(output.text, "hel");

        let second = stream.next().await.unwrap().unwrap();
        let Some(api::response_event::Type::ClientActions(actions)) = second.r#type else {
            panic!("expected client actions");
        };
        let Some(api::client_action::Action::AppendToMessageContent(append)) = actions
            .actions
            .into_iter()
            .next()
            .and_then(|action| action.action)
        else {
            panic!("expected append action");
        };
        assert_eq!(
            append.mask.unwrap().paths,
            vec!["agent_output.text".to_string()]
        );
        let Some(api::message::Message::AgentOutput(output)) = append.message.unwrap().message
        else {
            panic!("expected agent output append");
        };
        assert_eq!(output.text, "lo");
        let third = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            third.r#type,
            Some(api::response_event::Type::Finished(_))
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn handles_openai_stream_without_trailing_event_delimiter() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(r#"data: {"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}"#)
            .create_async()
            .await;

        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "test-model".to_string(),
            api_key: None,
            capabilities: Default::default(),
        };
        let body = ChatCompletionRequest {
            model: route.model.clone(),
            messages: vec![ChatMessage::user("hello".to_string())],
            stream: true,
            tools: vec![],
            tool_choice: None,
            parallel_tool_calls: None,
        };

        let mut stream = stream_chat_completion(
            route,
            body,
            "task-1".to_string(),
            "request-1".to_string(),
            vec![],
        );

        let first = stream.next().await.unwrap().unwrap();
        let Some(api::response_event::Type::ClientActions(actions)) = first.r#type else {
            panic!("expected client actions");
        };
        let Some(api::client_action::Action::AddMessagesToTask(add)) = actions
            .actions
            .into_iter()
            .next()
            .and_then(|action| action.action)
        else {
            panic!("expected add messages action");
        };
        let Some(api::message::Message::AgentOutput(output)) =
            add.messages.into_iter().next().unwrap().message
        else {
            panic!("expected agent output");
        };
        assert_eq!(output.text, "done");

        let second = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            second.r#type,
            Some(api::response_event::Type::Finished(_))
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn fetches_openai_compatible_model_ids_with_key() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_body(
                r#"{
                    "object": "list",
                    "data": [
                        { "id": "qwen3-coder", "object": "model" },
                        { "id": "llama-local", "object": "model" },
                        { "id": "qwen3-coder", "object": "model" },
                        { "object": "model" }
                    ]
                }"#,
            )
            .create_async()
            .await;

        let models = fetch_models(&format!("{}/v1", server.url()), Some("test-key")).await;

        assert_eq!(
            models.unwrap(),
            vec!["qwen3-coder".to_string(), "llama-local".to_string()]
        );
    }

    #[tokio::test]
    async fn fetches_openai_compatible_model_ids_without_key() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/v1/models")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(r#"{ "object": "list", "data": [{ "id": "local-model" }] }"#)
            .create_async()
            .await;

        let models = fetch_models(&format!("{}/v1", server.url()), None).await;

        assert_eq!(models.unwrap(), vec!["local-model".to_string()]);
    }
}
