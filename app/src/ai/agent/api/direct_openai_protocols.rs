//! Wire adapters for the locally configured provider. Conversation history and
//! tool execution remain in the shared direct-provider path.

use super::*;
use sha2::{Digest as _, Sha256};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_MAX_OUTPUT_TOKENS: u32 = 4_096;
const MAX_STREAM_ITEMS: usize = 4_096;
const RESPONSE_HISTORY_PREFIX: &str = "warp.direct_provider.response_history.v1;";

#[derive(Debug, Clone)]
pub(super) struct ProviderOutput {
    api_type: CustomApiType,
    response_id: String,
    output: Vec<Value>,
    prefix_fingerprint: Option<String>,
    retired_thinking: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ResponseHistory {
    api_type: CustomApiType,
    route_fingerprint: String,
    response_id: String,
    output: Option<Vec<Value>>,
    prefix_fingerprint: Option<String>,
    #[serde(default)]
    retired_thinking: Vec<String>,
}

fn history_route_fingerprint(route: &CustomProviderRoute) -> String {
    let identity = serde_json::to_vec(&(
        &route.provider_name,
        route.base_url.trim_end_matches('/'),
        &route.model,
    ))
    .expect("route identity is serializable");
    format!("{:x}", Sha256::digest(identity))
}

fn history_metadata(route: &CustomProviderRoute, output: &ProviderOutput, primary: bool) -> String {
    let history = ResponseHistory {
        api_type: output.api_type,
        route_fingerprint: history_route_fingerprint(route),
        response_id: output.response_id.clone(),
        output: primary.then(|| output.output.clone()),
        prefix_fingerprint: output.prefix_fingerprint.clone(),
        retired_thinking: output.retired_thinking.clone(),
    };
    format!(
        "{RESPONSE_HISTORY_PREFIX}{}",
        serde_json::to_string(&history).expect("response history is serializable")
    )
}

pub(super) fn history_from_message(
    message: &api::Message,
) -> anyhow::Result<Option<ResponseHistory>> {
    let Some(encoded) = message
        .server_message_data
        .strip_prefix(RESPONSE_HISTORY_PREFIX)
    else {
        return Ok(None);
    };
    serde_json::from_str(encoded)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("Local provider history metadata is invalid"))
}

pub(super) fn attach_response_history(
    route: &CustomProviderRoute,
    messages: &mut [api::Message],
    output: &ProviderOutput,
    primary: bool,
) {
    for (index, message) in messages.iter_mut().enumerate() {
        message.server_message_data = history_metadata(route, output, primary && index == 0);
    }
}

pub(super) fn history_update_action(
    route: &CustomProviderRoute,
    task_id: &str,
    message_id: &str,
    output: &ProviderOutput,
) -> api::ClientAction {
    api::ClientAction {
        action: Some(api::client_action::Action::UpdateTaskMessage(
            api::client_action::UpdateTaskMessage {
                task_id: task_id.to_string(),
                message: Some(api::Message {
                    id: message_id.to_string(),
                    server_message_data: history_metadata(route, output, true),
                    ..Default::default()
                }),
                mask: Some(prost_types::FieldMask {
                    paths: vec!["server_message_data".to_string()],
                }),
            },
        )),
    }
}

fn invalid(message: &'static str) -> AIApiError {
    AIApiError::Other(anyhow::anyhow!(message))
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, AIApiError> {
    value[key]
        .as_str()
        .ok_or_else(|| invalid("Direct provider response is missing a required string field"))
}

fn index(value: &Value, key: &str) -> Result<usize, AIApiError> {
    value[key]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value < MAX_STREAM_ITEMS)
        .ok_or_else(|| invalid("Direct provider stream contains an invalid content index"))
}

pub(super) fn authenticated_request(
    mut request: reqwest::RequestBuilder,
    api_type: CustomApiType,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    if api_type == CustomApiType::AnthropicMessages {
        request = request.header("anthropic-version", ANTHROPIC_VERSION);
        if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
            request = request.header("x-api-key", key);
        }
    } else if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    request
}

pub(super) fn completion_url(route: &CustomProviderRoute) -> String {
    let suffix = match route.api_type {
        CustomApiType::OpenAiCompatible => "chat/completions",
        CustomApiType::OpenAiResponses => "responses",
        CustomApiType::AnthropicMessages => "messages",
    };
    format!("{}/{suffix}", route.base_url.trim_end_matches('/'))
}

pub(super) fn request_body(
    route: &CustomProviderRoute,
    request: &ChatCompletionRequest,
) -> Result<Value, AIApiError> {
    match route.api_type {
        CustomApiType::OpenAiCompatible => serde_json::to_value(request)
            .map_err(|_| invalid("Failed to encode the direct provider request")),
        CustomApiType::OpenAiResponses => responses_request(route, request),
        CustomApiType::AnthropicMessages => anthropic_request(route, request),
    }
}

fn responses_request(
    route: &CustomProviderRoute,
    request: &ChatCompletionRequest,
) -> Result<Value, AIApiError> {
    let mut input = Vec::new();
    let route_fingerprint = history_route_fingerprint(route);
    let mut replayed = HashSet::new();
    for message in &request.messages {
        if let Some(history) = &message.response_history
            && history.api_type == CustomApiType::OpenAiResponses
            && history.route_fingerprint == route_fingerprint
        {
            if let Some(output) = &history.output {
                if replayed.insert(&history.response_id) {
                    input.extend(output.iter().cloned());
                }
                continue;
            }
            if replayed.contains(&history.response_id) {
                continue;
            }
        }
        if message.role == "tool" {
            input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.as_deref().filter(|id| !id.is_empty())
                    .ok_or_else(|| invalid("Local tool result is missing its call identifier"))?,
                "output": text_content(message.content.as_ref())?,
            }));
            continue;
        }
        if let Some(content) = &message.content {
            let content = match content {
                ChatMessageContent::Text(text) => Value::String(text.clone()),
                ChatMessageContent::Parts(parts) => Value::Array(parts.iter().map(|part| {
                    match part.kind {
                        "text" => Ok(json!({"type": "input_text", "text": part.text.as_deref().unwrap_or_default()})),
                        "image_url" => Ok(json!({"type": "input_image", "image_url": part.image_url.as_ref()
                            .ok_or_else(|| invalid("Local image content is missing its URL"))?.url})),
                        _ => Err(invalid("Local message contains unsupported content")),
                    }
                }).collect::<Result<Vec<_>, _>>()?),
            };
            input.push(json!({"role": message.role, "content": content}));
        }
        for call in &message.tool_calls {
            validate_openai_tool_call_envelope(call)
                .map_err(|_| invalid("Local history contains an invalid tool call"))?;
            input.push(json!({
                "type": "function_call", "call_id": call.id,
                "name": call.function.name, "arguments": call.function.arguments,
            }));
        }
    }
    let mut body = json!({
        "model": request.model, "input": input,
        "stream": request.stream, "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function", "name": tool.function.name,
                        "description": tool.function.description,
                        "parameters": tool.function.parameters,
                        // The shared tool schemas contain optional properties. Do not let
                        // Responses silently normalize them to strict, all-required input.
                        "strict": false,
                    })
                })
                .collect(),
        );
        if let Some(choice) = request.tool_choice {
            body["tool_choice"] = json!(choice);
        }
        if let Some(parallel) = request.parallel_tool_calls {
            body["parallel_tool_calls"] = json!(parallel);
        }
    }
    if !route.prompt_caching {
        // Unsupported endpoints must reject this setting instead of silently
        // reading or writing the provider's implicit prefix cache.
        body["prompt_cache_options"] = json!({"mode": "explicit"});
    }
    Ok(body)
}

fn text_content(content: Option<&ChatMessageContent>) -> Result<String, AIApiError> {
    match content {
        None => Ok(String::new()),
        Some(ChatMessageContent::Text(text)) => Ok(text.clone()),
        Some(ChatMessageContent::Parts(parts)) => parts
            .iter()
            .map(|part| {
                if part.kind == "text" {
                    Ok(part.text.as_deref().unwrap_or_default())
                } else {
                    Err(invalid("This local message requires plain text content"))
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join("\n")),
    }
}

fn anthropic_content(content: &ChatMessageContent) -> Result<Vec<Value>, AIApiError> {
    match content {
        ChatMessageContent::Text(text) => Ok((!text.is_empty())
            .then(|| json!({"type": "text", "text": text}))
            .into_iter()
            .collect()),
        ChatMessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part.kind {
                "text" => {
                    Ok(json!({"type": "text", "text": part.text.as_deref().unwrap_or_default()}))
                }
                "image_url" => {
                    let url = &part
                        .image_url
                        .as_ref()
                        .ok_or_else(|| invalid("Local image content is missing its URL"))?
                        .url;
                    let source = if let Some(data) = url.strip_prefix("data:") {
                        let (media_type, data) = data.split_once(";base64,").ok_or_else(|| {
                            invalid("Local image content has an invalid data URL")
                        })?;
                        if !is_supported_image_mime_type(media_type) {
                            return Err(invalid(
                                "Local image content has an unsupported media type",
                            ));
                        }
                        json!({"type": "base64", "media_type": media_type, "data": data})
                    } else if url.starts_with("https://") || url.starts_with("http://") {
                        json!({"type": "url", "url": url})
                    } else {
                        return Err(invalid("Local image content has an unsupported URL"));
                    };
                    Ok(json!({"type": "image", "source": source}))
                }
                _ => Err(invalid("Local message contains unsupported content")),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|blocks| {
                blocks
                    .into_iter()
                    .filter(|block| {
                        block["type"] != "text"
                            || block["text"].as_str().is_some_and(|text| !text.is_empty())
                    })
                    .collect()
            }),
    }
}

fn anthropic_request(
    route: &CustomProviderRoute,
    request: &ChatCompletionRequest,
) -> Result<Value, AIApiError> {
    anthropic_request_with_replay_policy(route, request).map(|(body, _)| body)
}

fn thinking_block(block: &Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("thinking" | "redacted_thinking")
    )
}

fn anthropic_prefix_fingerprint(system: &[Value], tools: &[Value], messages: &[Value]) -> String {
    let prefix =
        serde_json::to_vec(&(system, tools, messages)).expect("provider prefix is serializable");
    format!("{:x}", Sha256::digest(prefix))
}

fn append_anthropic_message(messages: &mut Vec<Value>, role: &str, mut content: Vec<Value>) {
    if content.is_empty() {
        return;
    }
    if let Some(previous) = messages
        .last_mut()
        .filter(|previous| previous["role"] == role)
    {
        previous["content"]
            .as_array_mut()
            .expect("constructed content array")
            .append(&mut content);
    } else {
        messages.push(json!({"role": role, "content": content}));
    }
}

fn anthropic_request_with_replay_policy(
    route: &CustomProviderRoute,
    request: &ChatCompletionRequest,
) -> Result<(Value, Vec<String>), AIApiError> {
    let mut system = Vec::new();
    for message in request
        .messages
        .iter()
        .filter(|message| message.role == "system")
    {
        let text = text_content(message.content.as_ref())?;
        if !text.is_empty() {
            system.push(json!({"type": "text", "text": text}));
        }
    }
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.function.name, "description": tool.function.description,
                "input_schema": tool.function.parameters,
            })
        })
        .collect();
    let route_fingerprint = history_route_fingerprint(route);
    let mut replayed = HashSet::new();
    // A prefix edit invalidates signed thinking. Record dropped batches in the
    // next local response so restoring an older prefix cannot create a gap in
    // the provider's preserved-thinking chain.
    let mut retired: std::collections::BTreeSet<String> = request
        .messages
        .iter()
        .filter_map(|message| message.response_history.as_ref())
        .filter(|history| history.api_type == CustomApiType::AnthropicMessages)
        .flat_map(|history| history.retired_thinking.iter().cloned())
        .collect();
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        if message.role == "system" {
            continue;
        }
        if let Some(history) = &message.response_history
            && history.api_type == CustomApiType::AnthropicMessages
        {
            if let Some(output) = &history.output {
                let bound = history.route_fingerprint == route_fingerprint;
                let prefix_matches = history.prefix_fingerprint.as_deref()
                    == Some(anthropic_prefix_fingerprint(&system, &tools, &messages).as_str());
                if output.iter().any(thinking_block) && (!bound || !prefix_matches) {
                    retired.insert(history.response_id.clone());
                }
                if bound {
                    if replayed.insert(&history.response_id) {
                        let content = output
                            .iter()
                            .filter(|block| {
                                !retired.contains(&history.response_id) || !thinking_block(block)
                            })
                            .cloned()
                            .collect();
                        append_anthropic_message(&mut messages, "assistant", content);
                    }
                    continue;
                }
            } else if history.route_fingerprint == route_fingerprint
                && replayed.contains(&history.response_id)
            {
                continue;
            }
        }
        let (role, content) = if message.role == "tool" {
            (
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.as_deref().filter(|id| !id.is_empty())
                        .ok_or_else(|| invalid("Local tool result is missing its call identifier"))?,
                    "content": text_content(message.content.as_ref())?,
                })],
            )
        } else {
            if !matches!(message.role, "user" | "assistant") {
                return Err(invalid("Local message contains an unsupported role"));
            }
            let mut content = message
                .content
                .as_ref()
                .map(anthropic_content)
                .transpose()?
                .unwrap_or_default();
            for call in &message.tool_calls {
                let input = validate_openai_tool_call_envelope(call)
                    .map_err(|_| invalid("Local history contains an invalid tool call"))?;
                content.push(json!({"type": "tool_use", "id": call.id,
                    "name": call.function.name, "input": input}));
            }
            (message.role, content)
        };
        // Parallel tool results stay in one user message before attached context.
        append_anthropic_message(&mut messages, role, content);
    }
    let mut body = json!({"model": request.model, "messages": messages,
        "stream": request.stream, "max_tokens": ANTHROPIC_MAX_OUTPUT_TOKENS});
    if !system.is_empty() {
        body["system"] = Value::Array(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(tools);
        if let Some(choice) = request.tool_choice {
            body["tool_choice"] = json!({"type": choice,
                "disable_parallel_tool_use": !request.parallel_tool_calls.unwrap_or(true)});
        }
    }
    if route.prompt_caching {
        body["cache_control"] = json!({"type": "ephemeral"});
    }
    Ok((body, retired.into_iter().collect()))
}

pub(super) fn bind_output_to_request(
    route: &CustomProviderRoute,
    request: &ChatCompletionRequest,
    output: &mut Option<ProviderOutput>,
) -> Result<(), AIApiError> {
    let Some(output) = output
        .as_mut()
        .filter(|output| output.api_type == CustomApiType::AnthropicMessages)
    else {
        return Ok(());
    };
    let (body, retired) = anthropic_request_with_replay_policy(route, request)?;
    let empty = Vec::new();
    output.prefix_fingerprint = Some(anthropic_prefix_fingerprint(
        body["system"].as_array().unwrap_or(&empty),
        body["tools"].as_array().unwrap_or(&empty),
        body["messages"]
            .as_array()
            .expect("constructed messages array"),
    ));
    output.retired_thinking = retired;
    Ok(())
}

pub(super) async fn send_request(
    route: &CustomProviderRoute,
    body: &ChatCompletionRequest,
) -> Result<reqwest::Response, AIApiError> {
    let response = authenticated_request(
        reqwest::Client::new()
            .post(completion_url(route))
            .json(&request_body(route, body)?),
        route.api_type,
        route.api_key.as_deref(),
    )
    .send()
    .await
    .map_err(|_| invalid("Failed to send the direct provider request"))?;
    if !response.status().is_success() {
        return Err(status_error(
            response,
            route.api_type == CustomApiType::OpenAiResponses && !route.prompt_caching,
        )
        .await);
    }
    Ok(response)
}

async fn status_error(mut response: reqwest::Response, explicit_cache: bool) -> AIApiError {
    let status = response.status();
    // Inspect a bounded error body solely to preserve the local compaction
    // overflow retry. Never return provider-controlled error text or secrets.
    let mut bytes = Vec::new();
    while bytes.len() < 65_536 {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                bytes.extend_from_slice(&chunk[..chunk.len().min(65_536 - bytes.len())])
            }
            _ => break,
        }
    }
    let body = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    let message = if matches!(status.as_u16(), 400 | 413)
        && ((body.contains("context")
            && ["token", "length", "maximum", "limit"]
                .iter()
                .any(|word| body.contains(word)))
            || body.contains("prompt is too long"))
    {
        "Direct provider rejected the request because its context token limit was exceeded."
    } else if status.as_u16() == 400
        && body.contains("thinking")
        && (body.contains("signature")
            || body.contains("prefix")
            || body.contains("cannot be modified"))
    {
        "Direct provider rejected preserved thinking after a conversation-prefix or signature mismatch. Start a new conversation or restore its original system prompt, tools, and history."
    } else if explicit_cache && status.as_u16() == 400 {
        "Direct provider rejected the Responses request. Disabling prompt caching requires an endpoint and model that support prompt_cache_options explicit mode (OpenAI GPT-5.6 or later)."
    } else {
        "Direct provider rejected the request. Check the selected protocol, model, endpoint, and credentials."
    };
    AIApiError::ErrorStatus(status, message.to_string())
}

pub(super) async fn fetch_models(
    base_url: &str,
    api_key: Option<&str>,
    api_type: CustomApiType,
) -> Result<Vec<String>, AIApiError> {
    let client = reqwest::Client::new();
    let mut models = Vec::new();
    let mut seen_models = HashSet::new();
    let mut seen_cursors = HashSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut request = client.get(models_url(base_url));
        if let Some(cursor) = &cursor {
            request = request.query(&[("after_id", cursor)]);
        }
        let response = authenticated_request(request, api_type, api_key)
            .send()
            .await
            .map_err(|_| invalid("Failed to request direct provider models"))?;
        if !response.status().is_success() {
            return Err(status_error(response, false).await);
        }
        let response: ModelsResponse = response
            .json()
            .await
            .map_err(|_| invalid("Failed to decode the direct provider models response"))?;
        for model in response.data.into_iter().filter_map(|entry| entry.id) {
            let model = model.trim();
            if !model.is_empty() && seen_models.insert(model.to_string()) {
                models.push(model.to_string());
            }
        }
        if api_type != CustomApiType::AnthropicMessages || !response.has_more {
            return Ok(models);
        }
        let next_cursor = response
            .last_id
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| invalid("Direct provider model pagination is missing its cursor"))?;
        if !seen_cursors.insert(next_cursor.clone()) || seen_cursors.len() > 1_000 {
            return Err(invalid("Direct provider model pagination did not advance"));
        }
        cursor = Some(next_cursor);
    }
}

pub(super) async fn decode_http_response(
    api_type: CustomApiType,
    response: reqwest::Response,
) -> Result<ChatCompletionResponse, AIApiError> {
    let value = response
        .json()
        .await
        .map_err(|_| invalid("Failed to decode the direct provider response JSON"))?;
    decode_response(api_type, value)
}

pub(super) fn decode_response(
    api_type: CustomApiType,
    value: Value,
) -> Result<ChatCompletionResponse, AIApiError> {
    match api_type {
        CustomApiType::OpenAiCompatible => serde_json::from_value(value)
            .map_err(|_| invalid("Failed to decode the OpenAI-compatible completion")),
        CustomApiType::OpenAiResponses => decode_responses(&value),
        CustomApiType::AnthropicMessages => decode_anthropic(&value),
    }
}

fn normalized_completion(
    text: String,
    calls: Vec<OpenAIToolCall>,
) -> Result<ChatCompletionResponse, AIApiError> {
    let mut ids = HashSet::new();
    for call in &calls {
        validate_openai_tool_call_envelope(call)
            .map_err(|_| invalid("Direct provider returned an invalid tool call"))?;
        if !ids.insert(&call.id) {
            return Err(invalid(
                "Direct provider returned duplicate tool call identifiers",
            ));
        }
    }
    if text.trim().is_empty() && calls.is_empty() {
        return Err(invalid("Direct provider returned an empty completion"));
    }
    let finish_reason = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    Ok(ChatCompletionResponse {
        response_output: None,
        choices: vec![ChatChoice {
            message: ChatChoiceMessage {
                content: (!text.is_empty()).then_some(text),
                tool_calls: calls,
            },
            finish_reason: Some(finish_reason.to_string()),
        }],
    })
}

fn decode_responses(value: &Value) -> Result<ChatCompletionResponse, AIApiError> {
    match string(value, "status")? {
        "completed" => {}
        "incomplete" => {
            return Err(invalid(
                "Responses provider returned an incomplete response; output may have reached its token limit",
            ));
        }
        "failed" | "cancelled" => {
            return Err(invalid(
                "Responses provider failed or cancelled the response",
            ));
        }
        _ => {
            return Err(invalid(
                "Responses provider did not reach a completed terminal state",
            ));
        }
    }
    if !value["error"].is_null() {
        return Err(invalid("Responses provider returned an error"));
    }
    let output = value["output"]
        .as_array()
        .ok_or_else(|| invalid("Responses provider response is missing its output"))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for item in output {
        if item["status"]
            .as_str()
            .is_some_and(|status| status != "completed")
        {
            return Err(invalid(
                "Responses provider returned an incomplete output item",
            ));
        }
        match string(item, "type")? {
            "message" => {
                if string(item, "role")? != "assistant" {
                    return Err(invalid(
                        "Responses provider returned an invalid message role",
                    ));
                }
                for part in item["content"]
                    .as_array()
                    .ok_or_else(|| invalid("Responses provider message is missing content"))?
                {
                    match string(part, "type")? {
                        "output_text" => text.push_str(string(part, "text")?),
                        "refusal" => return Err(invalid("Responses provider refused the request")),
                        _ => {
                            return Err(invalid(
                                "Responses provider returned unsupported output content",
                            ));
                        }
                    }
                }
            }
            "function_call" => calls.push(OpenAIToolCall {
                id: string(item, "call_id")?.to_string(),
                kind: "function".to_string(),
                function: OpenAIFunctionCall {
                    name: string(item, "name")?.to_string(),
                    arguments: string(item, "arguments")?.to_string(),
                },
            }),
            "reasoning" => {}
            _ => {
                return Err(invalid(
                    "Responses provider returned an unsupported output item",
                ));
            }
        }
    }
    let mut completion = normalized_completion(text, calls)?;
    completion.response_output = Some(ProviderOutput {
        api_type: CustomApiType::OpenAiResponses,
        response_id: string(value, "id")?.to_string(),
        output: output.clone(),
        prefix_fingerprint: None,
        retired_thinking: Vec::new(),
    });
    Ok(completion)
}

fn decode_anthropic(value: &Value) -> Result<ChatCompletionResponse, AIApiError> {
    if string(value, "type")? != "message" || string(value, "role")? != "assistant" {
        return Err(invalid(
            "Anthropic provider returned an invalid message envelope",
        ));
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    let content = value["content"]
        .as_array()
        .ok_or_else(|| invalid("Anthropic provider response is missing content"))?;
    for block in content {
        match string(block, "type")? {
            "text" => text.push_str(string(block, "text")?),
            "tool_use" => {
                if !block["input"].is_object() {
                    return Err(invalid(
                        "Anthropic provider tool input must be a JSON object",
                    ));
                }
                calls.push(OpenAIToolCall {
                    id: string(block, "id")?.to_string(),
                    kind: "function".to_string(),
                    function: OpenAIFunctionCall {
                        name: string(block, "name")?.to_string(),
                        arguments: block["input"].to_string(),
                    },
                });
            }
            "thinking" => {
                string(block, "thinking")?;
                if string(block, "signature")?.is_empty() {
                    return Err(invalid("Anthropic thinking block is missing its signature"));
                }
            }
            "redacted_thinking" => {
                if string(block, "data")?.is_empty() {
                    return Err(invalid(
                        "Anthropic redacted thinking block is missing its data",
                    ));
                }
            }
            _ => {
                return Err(invalid(
                    "Anthropic provider returned an unsupported content block",
                ));
            }
        }
    }
    match string(value, "stop_reason")? {
        "end_turn" | "stop_sequence" if calls.is_empty() => {}
        "tool_use" if !calls.is_empty() => {}
        "max_tokens" | "model_context_window_exceeded" => {
            return Err(invalid(
                "Anthropic provider response reached its token limit before completion",
            ));
        }
        "refusal" => return Err(invalid("Anthropic provider refused the request")),
        _ => {
            return Err(invalid(
                "Anthropic provider returned an invalid or incomplete stop reason",
            ));
        }
    }
    let mut completion = normalized_completion(text, calls)?;
    completion.response_output = Some(ProviderOutput {
        api_type: CustomApiType::AnthropicMessages,
        response_id: string(value, "id")?.to_string(),
        output: content.clone(),
        prefix_fingerprint: None,
        retired_thinking: Vec::new(),
    });
    Ok(completion)
}

pub(super) fn text_chunk(text: Option<String>) -> ChatCompletionChunk {
    ChatCompletionChunk {
        choices: vec![ChatChunkChoice {
            delta: ChatChunkDelta {
                content: text,
                tool_calls: vec![],
            },
            finish_reason: None,
        }],
    }
}

pub(super) struct ProtocolStream {
    state: ProtocolState,
    emitted_text: String,
}

enum ProtocolState {
    Responses(ResponsesStream),
    Anthropic(AnthropicStream),
}

impl ProtocolStream {
    pub(super) fn new(api_type: CustomApiType) -> Option<Self> {
        let state = match api_type {
            CustomApiType::OpenAiCompatible => return None,
            CustomApiType::OpenAiResponses => ProtocolState::Responses(ResponsesStream::default()),
            CustomApiType::AnthropicMessages => {
                ProtocolState::Anthropic(AnthropicStream::default())
            }
        };
        Some(Self {
            state,
            emitted_text: String::new(),
        })
    }

    pub(super) fn apply_event(&mut self, event: &str) -> Result<Option<String>, AIApiError> {
        let Some(value) = parse_sse_event(event)? else {
            return Ok(None);
        };
        let text = match &mut self.state {
            ProtocolState::Responses(state) => state.apply(&value)?,
            ProtocolState::Anthropic(state) => state.apply(&value)?,
        };
        if let Some(text) = &text {
            self.emitted_text.push_str(text);
        }
        Ok(text)
    }

    pub(super) fn finish(
        self,
    ) -> Result<(ChatCompletionChunk, Option<ProviderOutput>), AIApiError> {
        let response = match self.state {
            ProtocolState::Responses(state) => state.completed,
            ProtocolState::Anthropic(state) => state.completed,
        }
        .ok_or_else(|| {
            invalid("Direct provider stream ended before its terminal completion event")
        })?;
        let response_output = response.response_output;
        let choice = response
            .choices
            .into_iter()
            .next()
            .expect("normalized single choice");
        let content = choice.message.content.unwrap_or_default();
        let remainder = content.strip_prefix(&self.emitted_text).ok_or_else(|| {
            invalid("Direct provider terminal text differs from its streamed content")
        })?;
        Ok((
            ChatCompletionChunk {
                choices: vec![ChatChunkChoice {
                    delta: ChatChunkDelta {
                        content: (!remainder.is_empty()).then(|| remainder.to_string()),
                        tool_calls: choice
                            .message
                            .tool_calls
                            .into_iter()
                            .enumerate()
                            .map(|(index, call)| StreamingToolCallDelta {
                                index,
                                id: Some(call.id),
                                kind: Some(call.kind),
                                function: Some(StreamingFunctionDelta {
                                    name: Some(call.function.name),
                                    arguments: Some(call.function.arguments),
                                }),
                            })
                            .collect(),
                    },
                    finish_reason: choice.finish_reason,
                }],
            },
            response_output,
        ))
    }
}

fn parse_sse_event(event: &str) -> Result<Option<Value>, AIApiError> {
    let mut name = None;
    let mut data = Vec::new();
    for line in event.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            name = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        } else if !line.starts_with("id:") && !line.starts_with("retry:") {
            return Err(invalid(
                "Direct provider SSE event contains an invalid field",
            ));
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(&data.join("\n"))
        .map_err(|_| invalid("Failed to decode direct provider SSE JSON"))?;
    let kind = string(&value, "type")?;
    if name.is_some_and(|name| name != kind) {
        return Err(invalid(
            "Direct provider SSE event type does not match its payload",
        ));
    }
    if matches!(
        kind,
        "error" | "response.failed" | "response.incomplete" | "response.cancelled"
    ) {
        return Err(invalid(
            "Direct provider stream failed or returned an incomplete response",
        ));
    }
    Ok(Some(value))
}

#[derive(Default)]
struct ResponsesStream {
    response_id: Option<String>,
    items: BTreeMap<usize, ResponseItem>,
    completed: Option<ChatCompletionResponse>,
}

struct ResponseItem {
    value: Value,
    text: BTreeMap<usize, TextPart>,
    arguments: String,
    arguments_done: bool,
    done: bool,
}

#[derive(Default)]
struct TextPart {
    text: String,
    done: bool,
}

impl ResponsesStream {
    fn response_id(&mut self, id: &str) -> Result<(), AIApiError> {
        if id.is_empty()
            || self
                .response_id
                .as_deref()
                .is_some_and(|previous| previous != id)
        {
            return Err(invalid("Responses stream changed its response identifier"));
        }
        self.response_id = Some(id.to_string());
        Ok(())
    }

    fn item_mut(&mut self, value: &Value) -> Result<&mut ResponseItem, AIApiError> {
        let item = self
            .items
            .get_mut(&index(value, "output_index")?)
            .ok_or_else(|| invalid("Responses stream referenced an unknown output index"))?;
        if item.done
            || value["item_id"]
                .as_str()
                .is_some_and(|id| item.value["id"].as_str() != Some(id))
        {
            return Err(invalid(
                "Responses stream referenced a closed or mismatched output item",
            ));
        }
        Ok(item)
    }

    fn apply(&mut self, value: &Value) -> Result<Option<String>, AIApiError> {
        if self.completed.is_some() {
            return Err(invalid(
                "Responses stream contained data after terminal completion",
            ));
        }
        if let Some(id) = value["response_id"].as_str() {
            self.response_id(id)?;
        }
        match string(value, "type")? {
            "response.created" | "response.in_progress" => {
                self.response_id(string(&value["response"], "id")?)?;
            }
            "response.output_item.added" => {
                let output_index = index(value, "output_index")?;
                if output_index != self.items.len() {
                    return Err(invalid(
                        "Responses stream skipped or repeated an output index",
                    ));
                }
                let item = &value["item"];
                if string(item, "id")?.is_empty()
                    || self
                        .items
                        .values()
                        .any(|previous| previous.value["id"] == item["id"])
                    || !matches!(
                        string(item, "type")?,
                        "message" | "function_call" | "reasoning"
                    )
                {
                    return Err(invalid("Responses stream returned an invalid output item"));
                }
                self.items.insert(
                    output_index,
                    ResponseItem {
                        value: item.clone(),
                        text: BTreeMap::new(),
                        arguments: item["arguments"].as_str().unwrap_or_default().to_string(),
                        arguments_done: false,
                        done: false,
                    },
                );
            }
            "response.content_part.added" => {
                let part_index = index(value, "content_index")?;
                let item = self.item_mut(value)?;
                if item.value["type"] != "message"
                    || part_index != item.text.len()
                    || value["part"]["type"] != "output_text"
                {
                    return Err(invalid("Responses stream returned invalid message content"));
                }
                let text = string(&value["part"], "text")?.to_string();
                item.text.insert(
                    part_index,
                    TextPart {
                        text: text.clone(),
                        done: false,
                    },
                );
                return Ok(Some(text));
            }
            "response.output_text.delta" | "response.output_text.done" => {
                let done = value["type"] == "response.output_text.done";
                let part_index = index(value, "content_index")?;
                let item = self.item_mut(value)?;
                let part = item
                    .text
                    .get_mut(&part_index)
                    .filter(|part| !part.done)
                    .ok_or_else(|| {
                        invalid("Responses stream referenced an unknown or closed text part")
                    })?;
                if done {
                    if string(value, "text")? != part.text {
                        return Err(invalid(
                            "Responses stream terminal text differs from its deltas",
                        ));
                    }
                    part.done = true;
                } else {
                    let delta = string(value, "delta")?;
                    part.text.push_str(delta);
                    return Ok(Some(delta.to_string()));
                }
            }
            "response.content_part.done" => {
                let part_index = index(value, "content_index")?;
                let item = self.item_mut(value)?;
                let part = item
                    .text
                    .get(&part_index)
                    .ok_or_else(|| invalid("Responses stream closed an unknown text part"))?;
                if !part.done
                    || value["part"]["type"] != "output_text"
                    || string(&value["part"], "text")? != part.text
                {
                    return Err(invalid(
                        "Responses stream closed an incomplete or mismatched text part",
                    ));
                }
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                let done = value["type"] == "response.function_call_arguments.done";
                let item = self.item_mut(value)?;
                if item.value["type"] != "function_call" || item.arguments_done {
                    return Err(invalid(
                        "Responses stream returned arguments for an invalid tool item",
                    ));
                }
                if done {
                    if string(value, "arguments")? != item.arguments {
                        return Err(invalid(
                            "Responses stream terminal tool arguments differ from its deltas",
                        ));
                    }
                    item.arguments_done = true;
                } else {
                    item.arguments.push_str(string(value, "delta")?);
                }
            }
            "response.output_item.done" => {
                let item = self.item_mut(value)?;
                let final_item = &value["item"];
                if final_item["id"] != item.value["id"] || final_item["type"] != item.value["type"]
                {
                    return Err(invalid("Responses stream changed its output item identity"));
                }
                match string(final_item, "type")? {
                    "function_call" => {
                        if !item.arguments_done
                            || string(final_item, "arguments")? != item.arguments
                            || final_item["call_id"] != item.value["call_id"]
                            || final_item["name"] != item.value["name"]
                        {
                            return Err(invalid(
                                "Responses stream closed an incomplete or mismatched tool call",
                            ));
                        }
                    }
                    "message" => {
                        let content = final_item["content"].as_array().ok_or_else(|| {
                            invalid("Responses stream message is missing content")
                        })?;
                        if content.len() != item.text.len() {
                            return Err(invalid("Responses stream omitted message content"));
                        }
                        for (index, part) in content.iter().enumerate() {
                            let streamed = &item.text[&index];
                            if !streamed.done
                                || part["type"] != "output_text"
                                || string(part, "text")? != streamed.text
                            {
                                return Err(invalid(
                                    "Responses stream closed an incomplete message",
                                ));
                            }
                        }
                    }
                    "reasoning" => {}
                    _ => return Err(invalid("Responses stream returned unsupported output")),
                }
                item.value = final_item.clone();
                item.done = true;
            }
            "response.completed" => {
                let response = &value["response"];
                self.response_id(string(response, "id")?)?;
                let output = response["output"]
                    .as_array()
                    .ok_or_else(|| invalid("Responses stream is missing terminal output"))?;
                if output.len() != self.items.len() || self.items.values().any(|item| !item.done) {
                    return Err(invalid(
                        "Responses stream completed with missing or unfinished output items",
                    ));
                }
                for (index, value) in output.iter().enumerate() {
                    let item = &self.items[&index].value;
                    if value["id"] != item["id"]
                        || value["type"] != item["type"]
                        || value["content"] != item["content"]
                        || value["arguments"] != item["arguments"]
                        || value["name"] != item["name"]
                        || value["call_id"] != item["call_id"]
                    {
                        return Err(invalid(
                            "Responses stream terminal output differs from its completed items",
                        ));
                    }
                }
                self.completed = Some(decode_responses(response)?);
            }
            "response.output_text.annotation.added"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done" => {}
            "response.refusal.delta" | "response.refusal.done" => {
                return Err(invalid("Responses provider refused the request"));
            }
            _ => {} // Providers may add metadata events without changing output.
        }
        Ok(None)
    }
}

#[derive(Default)]
struct AnthropicStream {
    started: bool,
    response_id: Option<String>,
    blocks: BTreeMap<usize, AnthropicBlock>,
    stop_reason: Option<String>,
    completed: Option<ChatCompletionResponse>,
}

struct AnthropicBlock {
    value: Value,
    input_json: String,
    done: bool,
}

impl AnthropicStream {
    fn apply(&mut self, value: &Value) -> Result<Option<String>, AIApiError> {
        let kind = string(value, "type")?;
        if kind == "ping" {
            return Ok(None);
        }
        if self.completed.is_some() {
            return Err(invalid(
                "Anthropic stream contained data after terminal completion",
            ));
        }
        if kind == "message_start" {
            if self.started
                || value["message"]["type"] != "message"
                || value["message"]["role"] != "assistant"
                || !value["message"]["content"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
            {
                return Err(invalid(
                    "Anthropic stream returned an invalid message start",
                ));
            }
            let id = string(&value["message"], "id")?;
            if id.is_empty() {
                return Err(invalid(
                    "Anthropic stream is missing its message identifier",
                ));
            }
            self.response_id = Some(id.to_string());
            self.started = true;
            return Ok(None);
        }
        if !self.started {
            return Err(invalid("Anthropic stream is missing its message start"));
        }
        match kind {
            "content_block_start" => {
                let block_index = index(value, "index")?;
                let block = &value["content_block"];
                if self.stop_reason.is_some()
                    || block_index != self.blocks.len()
                    || !matches!(
                        string(block, "type")?,
                        "text" | "tool_use" | "thinking" | "redacted_thinking"
                    )
                {
                    return Err(invalid(
                        "Anthropic stream skipped, repeated, or returned an invalid content block",
                    ));
                }
                let text = if block["type"] == "text" {
                    Some(string(block, "text")?.to_string())
                } else {
                    None
                };
                self.blocks.insert(
                    block_index,
                    AnthropicBlock {
                        value: block.clone(),
                        input_json: String::new(),
                        done: false,
                    },
                );
                return Ok(text);
            }
            "content_block_delta" => {
                let block = self
                    .blocks
                    .get_mut(&index(value, "index")?)
                    .filter(|block| !block.done && self.stop_reason.is_none())
                    .ok_or_else(|| {
                        invalid("Anthropic stream referenced an unknown or closed content block")
                    })?;
                let delta = &value["delta"];
                match string(delta, "type")? {
                    "text_delta" if block.value["type"] == "text" => {
                        let text = string(delta, "text")?;
                        let mut combined = string(&block.value, "text")?.to_string();
                        combined.push_str(text);
                        block.value["text"] = json!(combined);
                        return Ok(Some(text.to_string()));
                    }
                    "input_json_delta" if block.value["type"] == "tool_use" => {
                        block.input_json.push_str(string(delta, "partial_json")?);
                    }
                    "thinking_delta" | "signature_delta" if block.value["type"] == "thinking" => {
                        let field = if delta["type"] == "thinking_delta" {
                            "thinking"
                        } else {
                            "signature"
                        };
                        let mut combined = string(&block.value, field)?.to_string();
                        combined.push_str(string(delta, field)?);
                        block.value[field] = json!(combined);
                    }
                    "citations_delta" if block.value["type"] == "text" => {
                        if block.value["citations"].is_null() {
                            block.value["citations"] = json!([]);
                        }
                        let citations =
                            block.value["citations"].as_array_mut().ok_or_else(|| {
                                invalid("Anthropic stream has invalid citation content")
                            })?;
                        citations.push(delta["citation"].clone());
                    }
                    _ => {
                        return Err(invalid(
                            "Anthropic stream delta does not match its content block",
                        ));
                    }
                }
            }
            "content_block_stop" => {
                let block = self
                    .blocks
                    .get_mut(&index(value, "index")?)
                    .filter(|block| !block.done)
                    .ok_or_else(|| {
                        invalid("Anthropic stream closed an unknown or repeated content block")
                    })?;
                if block.value["type"] == "tool_use" && !block.input_json.is_empty() {
                    let input: Value = serde_json::from_str(&block.input_json).map_err(|_| {
                        invalid("Anthropic stream returned malformed tool input JSON")
                    })?;
                    if !input.is_object() {
                        return Err(invalid("Anthropic stream tool input must be a JSON object"));
                    }
                    block.value["input"] = input;
                }
                block.done = true;
            }
            "message_delta" => {
                if self.blocks.values().any(|block| !block.done) {
                    return Err(invalid(
                        "Anthropic stream stopped before its content blocks finished",
                    ));
                }
                if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                    if self
                        .stop_reason
                        .as_deref()
                        .is_some_and(|previous| previous != reason)
                    {
                        return Err(invalid("Anthropic stream changed its terminal stop reason"));
                    }
                    self.stop_reason = Some(reason.to_string());
                }
            }
            "message_stop" => {
                if self.blocks.values().any(|block| !block.done) || self.stop_reason.is_none() {
                    return Err(invalid(
                        "Anthropic stream stopped without completed content and stop reason",
                    ));
                }
                self.completed = Some(decode_anthropic(&json!({
                    "id": self.response_id, "type": "message", "role": "assistant", "stop_reason": self.stop_reason,
                    "content": self.blocks.values().map(|block| block.value.clone()).collect::<Vec<_>>(),
                }))?);
            }
            _ => {}
        }
        Ok(None)
    }
}
