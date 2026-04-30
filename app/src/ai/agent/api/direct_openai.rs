use std::sync::Arc;

use anyhow::Context as _;
use async_stream::stream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warp_multi_agent_api as api;

use crate::ai::agent::AIAgentInput;
use crate::ai::llms::LLMId;
use crate::server::server_api::AIApiError;

use super::ResponseStream;

const CUSTOM_MODEL_PREFIX: &str = "custom/";

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
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
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

pub(super) async fn generate(
    route: CustomProviderRoute,
    params: super::RequestParams,
) -> Result<ResponseStream, super::ConvertToAPITypeError> {
    let task_id = params
        .tasks
        .first()
        .map(|task| task.id.clone())
        .unwrap_or_else(|| "local-root-task".to_string());
    let request_id = Uuid::new_v4().to_string();
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let input = params.input.clone();

    let output_stream = stream! {
        yield Ok(api::ResponseEvent {
            r#type: Some(api::response_event::Type::Init(api::response_event::StreamInit {
                conversation_id: conversation_id.clone(),
                request_id: request_id.clone(),
                run_id: String::new(),
            })),
        });

        match request_chat_completion(&route, &input).await {
            Ok(text) => {
                yield Ok(client_actions_event(&task_id, &request_id, text));
                yield Ok(finished_event());
            }
            Err(error) => {
                yield Err(Arc::new(error));
            }
        }
    };

    Ok(Box::pin(output_stream))
}

async fn request_chat_completion(
    route: &CustomProviderRoute,
    input: &[AIAgentInput],
) -> Result<String, AIApiError> {
    let messages = openai_messages_from_inputs(input);
    if messages.is_empty() {
        return Err(AIApiError::Other(anyhow::anyhow!(
            "local_only custom provider request has no user-query input"
        )));
    }

    let body = ChatCompletionRequest {
        model: route.model.clone(),
        messages,
        stream: false,
    };

    let client = reqwest::Client::new();
    let mut request = client
        .post(chat_completions_url(&route.base_url))
        .json(&body);
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

    let response: ChatCompletionResponse = response
        .json()
        .await
        .context("failed to decode OpenAI-compatible chat completion response")?;
    let text = response
        .choices
        .into_iter()
        .find_map(|choice| choice.message.content)
        .unwrap_or_default();

    Ok(text)
}

fn openai_messages_from_inputs(input: &[AIAgentInput]) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    for item in input {
        match item {
            AIAgentInput::UserQuery { query, .. } => messages.push(ChatMessage {
                role: "user",
                content: query.clone(),
            }),
            AIAgentInput::AutoCodeDiffQuery { query, .. }
            | AIAgentInput::CreateNewProject { query, .. } => messages.push(ChatMessage {
                role: "user",
                content: query.clone(),
            }),
            AIAgentInput::SummarizeConversation { prompt } => {
                if let Some(prompt) = prompt {
                    messages.push(ChatMessage {
                        role: "user",
                        content: prompt.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    messages
}

fn client_actions_event(task_id: &str, request_id: &str, text: String) -> api::ResponseEvent {
    let message_id = Uuid::new_v4().to_string();
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(api::client_action::Action::AddMessagesToTask(
                        api::client_action::AddMessagesToTask {
                            task_id: task_id.to_string(),
                            messages: vec![api::Message {
                                id: message_id,
                                task_id: task_id.to_string(),
                                request_id: request_id.to_string(),
                                timestamp: None,
                                server_message_data: String::new(),
                                citations: vec![],
                                message: Some(api::message::Message::AgentOutput(
                                    api::message::AgentOutput { text },
                                )),
                            }],
                        },
                    )),
                }],
            },
        )),
    }
}

fn finished_event() -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::Finished(
            api::response_event::StreamFinished {
                token_usage: vec![],
                should_refresh_model_config: false,
                request_cost: None,
                conversation_usage_metadata: None,
                reason: Some(api::response_event::stream_finished::Reason::Done(
                    api::response_event::stream_finished::Done {},
                )),
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
