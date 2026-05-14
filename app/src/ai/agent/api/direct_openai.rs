use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use anyhow::Context as _;
use async_stream::stream;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;
use warp_multi_agent_api as api;

use crate::ai::agent::{
    AIAgentActionResult, AIAgentContext, AIAgentInput, AnyFileContent, MarkdownActionResult,
};
use crate::ai::llms::LLMId;
use crate::server::server_api::AIApiError;
use crate::settings::{normalize_custom_provider_env_var, CustomProviderConfig};
use ::ai::api_keys::ApiKeys;

use super::ResponseStream;

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
    let custom_model = parse_custom_model_id(model_id)?;
    let provider = providers
        .iter()
        .find(|provider| provider.name == custom_model.provider_name)?;

    Some(route_for_provider_model(
        provider,
        custom_model.model,
        api_keys,
    ))
}

pub(crate) fn default_custom_provider_route(
    providers: &[CustomProviderConfig],
    api_keys: &ApiKeys,
) -> Option<CustomProviderRoute> {
    providers.iter().find_map(|provider| {
        let model = provider.models.first()?.clone();
        Some(route_for_provider_model(provider, model, api_keys))
    })
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
        Self {
            role: "assistant",
            content: None,
            tool_calls: vec![tool_call],
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
    id: Option<String>,
    kind: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl StreamingToolCall {
    fn apply_delta(&mut self, delta: StreamingToolCallDelta) {
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
        let id = self.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let name = self.name.ok_or_else(|| {
            AIApiError::Other(anyhow::anyhow!(
                "OpenAI-compatible tool call was missing a function name"
            ))
        })?;
        Ok(OpenAIToolCall {
            id,
            kind: self.kind.unwrap_or_else(|| "function".to_string()),
            function: OpenAIFunctionCall {
                name,
                arguments: self.arguments,
            },
        })
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
    let task_id = response_task_id(&params);
    let needs_create_task = should_create_task(&params, &task_id);
    let request_id = Uuid::new_v4().to_string();
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let tools = openai_tools_for_supported_tools(&supported_tools);
    let input_messages = api_messages_from_inputs(&task_id, &request_id, &params.input);
    let chat_messages = openai_messages_from_params(&params, &tools);
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

        let mut completion_events = stream_chat_completion(
            route.clone(),
            body,
            task_id.clone(),
            request_id.clone(),
            prefix_actions,
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

fn stream_chat_completion(
    route: CustomProviderRoute,
    body: ChatCompletionRequest,
    task_id: String,
    request_id: String,
    mut prefix_actions: Vec<api::ClientAction>,
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
        match events_from_non_streaming_response(response, &task_id, &request_id, prefix_actions) {
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
    let mut buffer = String::new();
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
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(event_end) = buffer.find("\n\n") {
            let event = buffer[..event_end].to_string();
            buffer.drain(..event_end + 2);

            for event in apply_openai_sse_event(
                &event,
                &mut state,
                &task_id,
                &request_id,
                &mut prefix_actions,
            ) {
                yield Ok(event);
            }
        }
    }

    let residual_buffer_bytes = buffer.len();
    if !buffer.trim().is_empty() {
        let residual_event = std::mem::take(&mut buffer);
        for event in apply_openai_sse_event(
            &residual_event,
            &mut state,
            &task_id,
            &request_id,
            &mut prefix_actions,
        ) {
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
    let completed_tool_calls = match tool_calls
        .into_iter()
        .filter(|tool_call| tool_call.name.is_some() || tool_call.id.is_some())
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
            match api_tool_call_message(&task_id, &request_id, tool_call) {
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
        messages.push(api_tool_call_message(task_id, request_id, tool_call)?);
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

fn sse_data_payloads(event: &str) -> Vec<String> {
    event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .map(ToString::to_string)
        .collect()
}

fn apply_openai_sse_event(
    event: &str,
    state: &mut StreamCompletionState,
    task_id: &str,
    request_id: &str,
    prefix_actions: &mut Vec<api::ClientAction>,
) -> Vec<api::ResponseEvent> {
    let mut events = Vec::new();
    for data in sse_data_payloads(event) {
        if data.trim() == "[DONE]" {
            continue;
        }

        state.parsed_events += 1;
        let chunk: ChatCompletionChunk = match serde_json::from_str(&data) {
            Ok(chunk) => chunk,
            Err(error) => {
                log::warn!("Skipping malformed OpenAI-compatible stream event: {error}");
                continue;
            }
        };
        events.extend(state.apply_chunk(chunk, task_id, request_id, prefix_actions));
    }
    events
}

fn take_prefix_actions(prefix_actions: &mut Vec<api::ClientAction>) -> Vec<api::ClientAction> {
    std::mem::take(prefix_actions)
}

fn openai_messages_from_params(
    params: &super::RequestParams,
    tools: &[OpenAITool],
) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage::system(system_prompt(params, tools))];

    for task in &params.tasks {
        for message in &task.messages {
            messages.extend(openai_messages_from_api_message(message));
        }
    }

    messages.extend(openai_messages_from_inputs(&params.input));
    messages
}

fn openai_messages_from_api_message(message: &api::Message) -> Vec<ChatMessage> {
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
                Some(api::message::system_query::Type::FetchReviewComments(query)) => {
                    format!("Fetch review comments for {}", query.repo_path)
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
                tool_call_result_to_text(result),
            )]
        }
        Some(api::message::Message::AgentReasoning(reasoning)) => (!reasoning.reasoning.is_empty())
            .then(|| ChatMessage::assistant(format!("Reasoning: {}", reasoning.reasoning)))
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

fn openai_messages_from_inputs(input: &[AIAgentInput]) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    for item in input {
        match item {
            AIAgentInput::UserQuery { query, context, .. } => {
                messages.push(ChatMessage::user(with_context(
                    query.clone(),
                    context_text(context).as_deref(),
                )));
            }
            AIAgentInput::AutoCodeDiffQuery { query, context }
            | AIAgentInput::CreateNewProject { query, context } => {
                messages.push(ChatMessage::user(with_context(
                    query.clone(),
                    context_text(context).as_deref(),
                )));
            }
            AIAgentInput::CloneRepository {
                clone_repo_url,
                context,
            } => {
                messages.push(ChatMessage::user(with_context(
                    clone_repo_url.clone().into_url(),
                    context_text(context).as_deref(),
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
                    context_text(context).as_deref(),
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
            | AIAgentInput::FetchReviewComments { .. }
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

fn context_text(context: &[AIAgentContext]) -> Option<String> {
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
                parts.push(format!("Selected text:\n{}", truncate_context(text)));
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
                        file_context_content(rule)
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
                    file_context_content(file)
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
                    truncate_context(&block.output)
                ));
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(truncate_context(&parts.join("\n\n")))
    }
}

fn file_context_content(file: &crate::ai::agent::FileContext) -> String {
    match &file.content {
        AnyFileContent::StringContent(content) => truncate_context(content),
        AnyFileContent::BinaryContent(_) => "[binary file content omitted]".to_string(),
    }
}

fn truncate_context(text: &str) -> String {
    if text.len() <= MAX_CONTEXT_CHARS {
        return text.to_string();
    }
    let mut truncated = text
        .char_indices()
        .take_while(|(idx, _)| *idx < MAX_CONTEXT_CHARS)
        .map(|(_, ch)| ch)
        .collect::<String>();
    truncated.push_str("\n[truncated]");
    truncated
}

fn system_prompt(params: &super::RequestParams, tools: &[OpenAITool]) -> String {
    let mut prompt = String::from(
        "You are Warp Agent running inside the Warp terminal app. Warp is a real local harness, not a plain chat. \
Use the provided OpenAI tool-calling interface whenever you need shell access, file access, code search, MCP tools, or Warp skills. \
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
        let mcp_summary = mcp_context_summary(mcp_context);
        if !mcp_summary.is_empty() {
            prompt.push_str("\n\nMCP context:\n");
            prompt.push_str(&mcp_summary);
        }
    }

    prompt
}

fn mcp_context_summary(context: &crate::ai::agent::MCPContext) -> String {
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

    truncate_context(&lines.join("\n"))
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
                            .map(|(key, attachment)| (key.clone(), attachment.clone().into()))
                            .collect(),
                        mode: Some((*user_query_mode).into()),
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
    let input: api::request::input::user_inputs::user_input::Input =
        result.clone().try_into().ok()?;
    let api::request::input::user_inputs::user_input::Input::ToolCallResult(result) = input else {
        return None;
    };

    Some(api::message::ToolCallResult {
        tool_call_id: result.tool_call_id,
        context: None,
        result: result
            .result
            .map(request_tool_result_to_message_tool_result),
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

fn api_tool_call_message(
    task_id: &str,
    request_id: &str,
    tool_call: OpenAIToolCall,
) -> Result<api::Message, AIApiError> {
    log::info!(
        "OpenAI-compatible custom provider requested Warp tool call: {}",
        tool_call.function.name
    );
    let tool = api_tool_from_openai_tool_call(&tool_call)?;
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
    let raw_arguments = tool_call.function.arguments.trim();
    let args: Value = if raw_arguments.is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw_arguments).with_context(|| {
            format!(
                "failed to decode arguments for OpenAI-compatible tool `{}`",
                tool_call.function.name
            )
        })?
    };

    match tool_call.function.name.as_str() {
        "run_shell_command" => {
            let command = required_string(&args, "command")?;
            let inferred_flags = infer_shell_command_flags(&command);
            let requested_wait_until_completion =
                optional_bool(&args, "wait_until_completion").unwrap_or(true);
            if !requested_wait_until_completion {
                log::warn!(
                    "OpenAI-compatible provider requested wait_until_completion=false for run_shell_command; forcing completion wait because the direct provider path does not expose long-running command polling tools"
                );
            }
            Ok(api::message::tool_call::Tool::RunShellCommand(
                api::message::tool_call::RunShellCommand {
                    command,
                    is_read_only: optional_bool(&args, "is_read_only")
                        .unwrap_or(inferred_flags.is_read_only),
                    uses_pager: optional_bool(&args, "uses_pager").unwrap_or(false),
                    citations: vec![],
                    is_risky: optional_bool(&args, "is_risky").unwrap_or(inferred_flags.is_risky),
                    wait_until_complete_value: Some(
                        api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(
                            true,
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
                path_filters: optional_string_array(&args, "path_filters"),
                codebase_path: optional_string(&args, "codebase_path").unwrap_or_default(),
            },
        )),
        "grep" => Ok(api::message::tool_call::Tool::Grep(
            api::message::tool_call::Grep {
                queries: string_array_or_single(&args, "queries", "query")?,
                path: optional_string(&args, "path").unwrap_or_default(),
            },
        )),
        "file_glob" => Ok(api::message::tool_call::Tool::FileGlobV2(
            api::message::tool_call::FileGlobV2 {
                patterns: string_array_or_single(&args, "patterns", "pattern")?,
                search_dir: optional_string(&args, "search_dir")
                    .or_else(|| optional_string(&args, "path"))
                    .unwrap_or_default(),
                max_matches: optional_i32(&args, "max_matches").unwrap_or_default(),
                max_depth: optional_i32(&args, "max_depth").unwrap_or_default(),
                min_depth: optional_i32(&args, "min_depth").unwrap_or_default(),
            },
        )),
        "read_mcp_resource" => Ok(api::message::tool_call::Tool::ReadMcpResource(
            api::message::tool_call::ReadMcpResource {
                uri: required_string(&args, "uri")?,
                server_id: optional_string(&args, "server_id").unwrap_or_default(),
            },
        )),
        "call_mcp_tool" => {
            let input = args.get("args").cloned().unwrap_or_else(|| json!({}));
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
                    name: required_string(&args, "name")
                        .or_else(|_| required_string(&args, "tool"))?,
                    args: Some(tool_args),
                    server_id: optional_string(&args, "server_id").unwrap_or_default(),
                },
            ))
        }
        "read_skill" => {
            let skill_path = optional_string(&args, "skill_path");
            let bundled_skill_id = optional_string(&args, "bundled_skill_id");
            let skill_reference = if let Some(skill_path) = skill_path.filter(|s| !s.is_empty()) {
                Some(api::message::tool_call::read_skill::SkillReference::SkillPath(skill_path))
            } else {
                bundled_skill_id
                    .filter(|id| !id.is_empty())
                    .map(api::message::tool_call::read_skill::SkillReference::BundledSkillId)
            };
            Ok(api::message::tool_call::Tool::ReadSkill(
                api::message::tool_call::ReadSkill {
                    skill_reference,
                    name: optional_string(&args, "name").unwrap_or_default(),
                },
            ))
        }
        "apply_file_diffs" => Ok(api::message::tool_call::Tool::ApplyFileDiffs(
            apply_file_diffs_arg(&args)?,
        )),
        other => Err(AIApiError::Other(anyhow::anyhow!(
            "OpenAI-compatible provider called unsupported Warp tool `{other}`"
        ))),
    }
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
    Ok(api::message::tool_call::read_files::File {
        name: required_string(value, "name").or_else(|_| required_string(value, "path"))?,
        line_ranges: value
            .get("line_ranges")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|range| {
                Ok(api::FileContentLineRange {
                    start: optional_u32(range, "start").unwrap_or_default(),
                    end: optional_u32(range, "end").unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
    })
}

fn apply_file_diffs_arg(
    args: &Value,
) -> Result<api::message::tool_call::ApplyFileDiffs, AIApiError> {
    Ok(api::message::tool_call::ApplyFileDiffs {
        summary: optional_string(args, "summary").unwrap_or_default(),
        diffs: args
            .get("diffs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|diff| {
                Ok(api::message::tool_call::apply_file_diffs::FileDiff {
                    file_path: required_string(diff, "file_path")
                        .or_else(|_| required_string(diff, "path"))?,
                    search: required_string(diff, "search")?,
                    replace: required_string(diff, "replace")?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
        new_files: args
            .get("new_files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|file| {
                Ok(api::message::tool_call::apply_file_diffs::NewFile {
                    file_path: required_string(file, "file_path")
                        .or_else(|_| required_string(file, "path"))?,
                    content: required_string(file, "content")?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
        deleted_files: args
            .get("deleted_files")
            .or_else(|| args.get("delete_files"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|file| {
                Ok(api::message::tool_call::apply_file_diffs::DeleteFile {
                    file_path: required_string(file, "file_path")
                        .or_else(|_| required_string(file, "path"))?,
                })
            })
            .collect::<Result<Vec<_>, AIApiError>>()?,
        v4a_updates: vec![],
    })
}

fn required_string(value: &Value, key: &str) -> Result<String, AIApiError> {
    optional_string(value, key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AIApiError::Other(anyhow::anyhow!("missing string argument `{key}`")))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn optional_bool(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn optional_i32(value: &Value, key: &str) -> Option<i32> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
}

fn optional_u32(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, AIApiError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| AIApiError::Other(anyhow::anyhow!("missing array argument `{key}`")))
}

fn optional_string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn string_array_or_single(
    value: &Value,
    array_key: &str,
    single_key: &str,
) -> Result<Vec<String>, AIApiError> {
    let values = optional_string_array(value, array_key);
    if !values.is_empty() {
        return Ok(values);
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

fn tool_call_result_to_text(result: &api::message::ToolCallResult) -> String {
    serde_json::to_string(&format!("{:?}", result.result)).unwrap_or_default()
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

fn openai_tools_for_supported_tools(supported_tools: &[api::ToolType]) -> Vec<OpenAITool> {
    let supported = supported_tools.iter().copied().collect::<HashSet<_>>();
    let mut tools = Vec::new();

    if supported.contains(&api::ToolType::RunShellCommand) {
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
                            "description": "Use true. The OpenAI-compatible direct provider path waits for command completion before returning the tool result."
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
            name: "local-openai".to_string(),
            base_url: "http://localhost:1234/v1".to_string(),
            models: vec!["qwen3-coder".to_string()],
            api_key_env_var: Some("LOCAL_OPENAI_API_KEY".to_string()),
            api_type: Default::default(),
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
    async fn fetches_openai_compatible_model_ids() {
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
}
