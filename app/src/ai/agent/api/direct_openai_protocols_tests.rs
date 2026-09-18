use super::*;
use mockito::Matcher;

fn protocol_route(api_type: CustomApiType, base_url: String) -> CustomProviderRoute {
    CustomProviderRoute {
        provider_name: "local".to_string(),
        base_url,
        model: "model".to_string(),
        api_key: None,
        api_type,
        prompt_caching: true,
        capabilities: Default::default(),
    }
}

fn call(id: &str, name: &str, arguments: Value) -> OpenAIToolCall {
    OpenAIToolCall {
        id: id.to_string(),
        kind: "function".to_string(),
        function: OpenAIFunctionCall {
            name: name.to_string(),
            arguments: arguments.to_string(),
        },
    }
}

fn protocol_request(messages: Vec<ChatMessage>) -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: "model".to_string(),
        messages,
        stream: true,
        tools: openai_tools_for_supported_tools(&[
            api::ToolType::RunShellCommand,
            api::ToolType::CallMcpTool,
        ]),
        tool_choice: Some("auto"),
        parallel_tool_calls: Some(true),
    }
}

fn protocol_endpoint(api_type: CustomApiType) -> &'static str {
    match api_type {
        CustomApiType::OpenAiCompatible => "/v1/chat/completions",
        CustomApiType::OpenAiResponses => "/v1/responses",
        CustomApiType::AnthropicMessages => "/v1/messages",
    }
}

fn responses_message(text: &str) -> Value {
    json!({"type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": text, "annotations": []}]})
}

fn responses_call(index: usize, call: &OpenAIToolCall) -> Value {
    json!({"type": "function_call", "id": format!("fc_{index}"), "status": "completed",
        "call_id": call.id, "name": call.function.name, "arguments": call.function.arguments})
}

fn protocol_response(api_type: CustomApiType, text: &str, calls: &[OpenAIToolCall]) -> Value {
    match api_type {
        CustomApiType::OpenAiCompatible => json!({"choices": [{"message": {
            "content": text, "tool_calls": calls}, "finish_reason": if calls.is_empty() {"stop"} else {"tool_calls"}}]}),
        CustomApiType::OpenAiResponses => {
            let mut output = Vec::new();
            if !text.is_empty() {
                output.push(responses_message(text));
            }
            output.extend(
                calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| responses_call(index, call)),
            );
            json!({"id": "resp_1", "status": "completed", "output": output})
        }
        CustomApiType::AnthropicMessages => {
            let mut content = Vec::new();
            if !text.is_empty() {
                content.push(json!({"type": "text", "text": text}));
            }
            content.extend(calls.iter().map(|call| json!({"type": "tool_use", "id": call.id,
                "name": call.function.name, "input": serde_json::from_str::<Value>(&call.function.arguments).unwrap()})));
            json!({"id": "msg_1", "type": "message", "role": "assistant", "content": content,
                "stop_reason": if calls.is_empty() {"end_turn"} else {"tool_use"}})
        }
    }
}

fn protocol_sse_values(
    api_type: CustomApiType,
    text: &str,
    calls: &[OpenAIToolCall],
) -> Vec<Value> {
    let response = protocol_response(api_type, text, calls);
    let mut events = Vec::new();
    match api_type {
        CustomApiType::OpenAiResponses => {
            events.push(json!({"type": "response.created", "response": {"id": "resp_1", "status": "in_progress"}}));
            for (index, item) in response["output"].as_array().unwrap().iter().enumerate() {
                let mut initial = item.clone();
                initial["status"] = json!("in_progress");
                if item["type"] == "message" {
                    initial["content"] = json!([]);
                } else {
                    initial["arguments"] = json!("");
                }
                events.push(json!({"type": "response.output_item.added", "output_index": index, "item": initial}));
                if item["type"] == "message" {
                    events.push(json!({"type": "response.content_part.added", "output_index": index,
                        "item_id": item["id"], "content_index": 0, "part": {"type": "output_text", "text": ""}}));
                    for delta in text.chars() {
                        events.push(
                            json!({"type": "response.output_text.delta", "output_index": index,
                            "item_id": item["id"], "content_index": 0, "delta": delta.to_string()}),
                        );
                    }
                    events.push(
                        json!({"type": "response.output_text.done", "output_index": index,
                        "item_id": item["id"], "content_index": 0, "text": text}),
                    );
                    events.push(
                        json!({"type": "response.content_part.done", "output_index": index,
                        "item_id": item["id"], "content_index": 0, "part": item["content"][0]}),
                    );
                } else {
                    let arguments = item["arguments"].as_str().unwrap();
                    for delta in arguments.as_bytes().chunks(3) {
                        events.push(json!({"type": "response.function_call_arguments.delta", "output_index": index,
                            "item_id": item["id"], "delta": std::str::from_utf8(delta).unwrap()}));
                    }
                    events.push(json!({"type": "response.function_call_arguments.done", "output_index": index,
                        "item_id": item["id"], "arguments": arguments}));
                }
                events.push(json!({"type": "response.output_item.done", "output_index": index, "item": item}));
            }
            events.push(json!({"type": "response.completed", "response": response}));
        }
        CustomApiType::AnthropicMessages => {
            events.push(json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}}));
            for (index, block) in response["content"].as_array().unwrap().iter().enumerate() {
                let mut initial = block.clone();
                if block["type"] == "text" {
                    initial["text"] = json!("");
                } else {
                    initial["input"] = json!({});
                }
                events.push(json!({"type": "content_block_start", "index": index, "content_block": initial}));
                if block["type"] == "text" {
                    for delta in text.chars() {
                        events.push(json!({"type": "content_block_delta", "index": index,
                            "delta": {"type": "text_delta", "text": delta.to_string()}}));
                    }
                } else {
                    let arguments = block["input"].to_string();
                    for delta in arguments.as_bytes().chunks(3) {
                        events.push(json!({"type": "content_block_delta", "index": index,
                            "delta": {"type": "input_json_delta", "partial_json": std::str::from_utf8(delta).unwrap()}}));
                    }
                }
                events.push(json!({"type": "content_block_stop", "index": index}));
            }
            events.push(
                json!({"type": "message_delta", "delta": {"stop_reason": response["stop_reason"]}}),
            );
            events.push(json!({"type": "message_stop"}));
        }
        CustomApiType::OpenAiCompatible => unreachable!(),
    }
    events
}

fn sse_wire(events: &[Value]) -> String {
    events
        .iter()
        .map(|value| {
            format!(
                "event: {}\r\ndata: {value}\r\n\r\n",
                value["type"].as_str().unwrap()
            )
        })
        .collect()
}

fn new_protocols() -> [CustomApiType; 2] {
    [
        CustomApiType::OpenAiResponses,
        CustomApiType::AnthropicMessages,
    ]
}

#[test]
fn direct_protocol_route_keeps_identity_protocol_and_cache_preference() {
    for api_type in new_protocols() {
        let provider = CustomProviderConfig {
            name: "identity".into(),
            base_url: "http://localhost:1234/v1".into(),
            models: vec!["model".into()],
            api_type,
            prompt_caching: false,
            ..Default::default()
        };
        let route = resolve_custom_provider_route(
            "custom/identity/model",
            &[provider],
            &ApiKeys::default(),
        )
        .unwrap();
        assert_eq!(route.provider_name, "identity");
        assert_eq!(route.api_type, api_type);
        assert!(!route.prompt_caching);
    }
}

#[test]
fn direct_protocol_requests_preserve_history_tools_images_and_parallel_results() {
    let calls = vec![
        call("one", "run_shell_command", json!({"command": "pwd"})),
        call(
            "two",
            "call_mcp_tool",
            json!({"server_id": "local", "name": "inspect", "args": {}}),
        ),
    ];
    let image = OpenAIContentPart {
        kind: "image_url",
        text: None,
        image_url: Some(OpenAIImageUrl {
            url: "data:image/png;base64,aW1hZ2U=".to_string(),
        }),
    };
    let request = protocol_request(vec![
        ChatMessage::system("Stable system".to_string()),
        ChatMessage::user_parts(vec![text_content_part("Inspect".to_string()), image]),
        ChatMessage::assistant("Checking".to_string()),
        ChatMessage::assistant_tool_calls(calls),
        ChatMessage::tool("one".to_string(), "shell result".to_string()),
        ChatMessage::tool("two".to_string(), "mcp result".to_string()),
        ChatMessage::user("Attached context".to_string()),
    ]);
    let response_body = protocols::request_body(
        &protocol_route(
            CustomApiType::OpenAiResponses,
            "http://localhost/v1/".into(),
        ),
        &request,
    )
    .unwrap();
    assert_eq!(response_body["store"], false);
    assert!(response_body.get("previous_response_id").is_none());
    assert!(response_body.get("conversation").is_none());
    assert_eq!(response_body["input"][0]["role"], "system");
    assert_eq!(
        response_body["input"][1]["content"][1]["type"],
        "input_image"
    );
    assert_eq!(response_body["input"][3]["type"], "function_call");
    assert_eq!(response_body["input"][4]["call_id"], "two");
    assert_eq!(
        response_body["input"][5],
        json!({"type": "function_call_output", "call_id": "one", "output": "shell result"})
    );
    assert_eq!(response_body["input"][6]["call_id"], "two");
    assert_eq!(response_body["tools"][0]["strict"], false);
    assert!(
        response_body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "call_mcp_tool")
    );
    assert_eq!(response_body["parallel_tool_calls"], true);

    let anthropic = protocols::request_body(
        &protocol_route(
            CustomApiType::AnthropicMessages,
            "http://localhost/v1/".into(),
        ),
        &request,
    )
    .unwrap();
    assert_eq!(anthropic["system"][0]["text"], "Stable system");
    assert_eq!(
        anthropic["messages"][0]["content"][1],
        json!({"type": "image", "source": {
        "type": "base64", "media_type": "image/png", "data": "aW1hZ2U="}})
    );
    assert_eq!(anthropic["messages"][1]["role"], "assistant");
    assert_eq!(anthropic["messages"][1]["content"][1]["type"], "tool_use");
    let results = &anthropic["messages"][2];
    assert_eq!(results["role"], "user");
    assert_eq!(results["content"][0]["tool_use_id"], "one");
    assert_eq!(results["content"][1]["tool_use_id"], "two");
    assert_eq!(results["content"][2]["text"], "Attached context");
    assert_eq!(anthropic["tool_choice"]["disable_parallel_tool_use"], false);
    assert!(anthropic["max_tokens"].as_u64().unwrap() > 0);
    assert!(anthropic["tools"][0]["input_schema"].is_object());
}

#[test]
fn direct_protocol_caching_changes_prefix_policy_without_answer_storage() {
    let request = protocol_request(vec![ChatMessage::user("Hello".to_string())]);
    for api_type in [
        CustomApiType::OpenAiCompatible,
        CustomApiType::OpenAiResponses,
        CustomApiType::AnthropicMessages,
    ] {
        let mut route = protocol_route(api_type, "http://localhost/v1".into());
        let enabled = protocols::request_body(&route, &request).unwrap();
        route.prompt_caching = false;
        let disabled = protocols::request_body(&route, &request).unwrap();
        match api_type {
            CustomApiType::OpenAiCompatible => assert_eq!(enabled, disabled),
            CustomApiType::OpenAiResponses => {
                assert!(enabled.get("prompt_cache_options").is_none());
                assert_eq!(
                    disabled["prompt_cache_options"],
                    json!({"mode": "explicit"})
                );
                assert_eq!(disabled["store"], false);
            }
            CustomApiType::AnthropicMessages => {
                assert_eq!(enabled["cache_control"], json!({"type": "ephemeral"}));
                assert!(disabled.get("cache_control").is_none());
            }
        }
    }
}

#[test]
fn direct_protocol_auth_uses_only_the_selected_protocols_headers() {
    for api_type in [
        CustomApiType::OpenAiCompatible,
        CustomApiType::OpenAiResponses,
        CustomApiType::AnthropicMessages,
    ] {
        for key in [None, Some(""), Some("synthetic-credential-fixture")] {
            let request = protocols::authenticated_request(
                reqwest::Client::new().get("http://localhost/v1/models"),
                api_type,
                key,
            )
            .build()
            .unwrap();
            let headers = request.headers();
            let has_key = key.is_some_and(|key| !key.is_empty());
            assert_eq!(
                headers.contains_key("authorization"),
                has_key && api_type != CustomApiType::AnthropicMessages
            );
            assert_eq!(
                headers.contains_key("x-api-key"),
                has_key && api_type == CustomApiType::AnthropicMessages
            );
            assert_eq!(
                headers.contains_key("anthropic-version"),
                api_type == CustomApiType::AnthropicMessages
            );
        }
    }
}

#[tokio::test]
async fn direct_protocol_complete_text_uses_selected_endpoint_and_json_contract() {
    for api_type in new_protocols() {
        let mut server = mockito::Server::new_async().await;
        let mut route = protocol_route(api_type, format!("{}/v1", server.url()));
        route.api_key = Some("synthetic-credential-fixture".to_string());
        let mock = server
            .mock("POST", protocol_endpoint(api_type))
            .match_header(
                if api_type == CustomApiType::AnthropicMessages {
                    "x-api-key"
                } else {
                    "authorization"
                },
                if api_type == CustomApiType::AnthropicMessages {
                    "synthetic-credential-fixture"
                } else {
                    "Bearer synthetic-credential-fixture"
                },
            )
            .match_body(Matcher::PartialJson(
                json!({"model": "model", "stream": false}),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(protocol_response(api_type, "direct answer", &[]).to_string())
            .create_async()
            .await;
        assert_eq!(
            complete_text(route, "system".into(), "user".into())
                .await
                .unwrap(),
            "direct answer"
        );
        mock.assert_async().await;
    }
}

fn extract_output(events: Vec<api::ResponseEvent>) -> (String, Vec<api::message::ToolCall>, usize) {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut finished = 0;
    for event in events {
        match event.r#type {
            Some(api::response_event::Type::ClientActions(actions)) => {
                for action in actions.actions {
                    match action.action {
                        Some(api::client_action::Action::AddMessagesToTask(add)) => {
                            for message in add.messages {
                                match message.message {
                                    Some(api::message::Message::AgentOutput(output)) => {
                                        text.push_str(&output.text)
                                    }
                                    Some(api::message::Message::ToolCall(call)) => calls.push(call),
                                    _ => {}
                                }
                            }
                        }
                        Some(api::client_action::Action::AppendToMessageContent(append)) => {
                            if let Some(api::message::Message::AgentOutput(output)) =
                                append.message.and_then(|message| message.message)
                            {
                                text.push_str(&output.text);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some(api::response_event::Type::Finished(_)) => finished += 1,
            _ => {}
        }
    }
    (text, calls, finished)
}

#[tokio::test]
async fn direct_protocol_generate_json_and_sse_preserve_tools_and_shell_completion() {
    let tool_calls = vec![
        call(
            "shell",
            "run_shell_command",
            json!({"command": "pwd", "wait_until_completion": false}),
        ),
        call(
            "mcp",
            "call_mcp_tool",
            json!({"server_id": "local", "name": "inspect", "args": {"value": 1}}),
        ),
    ];
    for api_type in new_protocols() {
        for streaming in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let body = if streaming {
                sse_wire(&protocol_sse_values(api_type, "Hi 🦀", &tool_calls))
            } else {
                protocol_response(api_type, "Hi 🦀", &tool_calls).to_string()
            };
            let mock = server
                .mock("POST", protocol_endpoint(api_type))
                .with_status(200)
                .with_header(
                    "content-type",
                    if streaming {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                )
                .with_body(body)
                .create_async()
                .await;
            let events = generate(
                protocol_route(api_type, format!("{}/v1", server.url())),
                super::super::RequestParams::new_for_test(),
                vec![api::ToolType::RunShellCommand, api::ToolType::CallMcpTool],
            )
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
            let (text, calls, finished) = extract_output(events);
            assert_eq!(text, "Hi 🦀");
            assert_eq!(finished, 1);
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].tool_call_id, "shell");
            let Some(api::message::tool_call::Tool::RunShellCommand(command)) = &calls[0].tool
            else {
                panic!("expected shell tool")
            };
            assert!(matches!(command.wait_until_complete_value,
                Some(api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(true))));
            assert!(matches!(
                calls[1].tool,
                Some(api::message::tool_call::Tool::CallMcpTool(_))
            ));
            mock.assert_async().await;
        }
    }
}

#[test]
fn direct_protocol_sse_survives_bytewise_utf8_and_crlf_fragmentation() {
    for api_type in new_protocols() {
        let mut decoder = protocols::ProtocolStream::new(api_type).unwrap();
        let wire = sse_wire(&protocol_sse_values(api_type, "Привет 🦀", &[]));
        let mut buffer = Vec::new();
        let mut text = String::new();
        for byte in wire.bytes() {
            buffer.push(byte);
            while let Some((end, delimiter)) = sse_event_end(&buffer) {
                let event = String::from_utf8(buffer.drain(..end).collect()).unwrap();
                buffer.drain(..delimiter);
                if let Some(delta) = decoder.apply_event(&event).unwrap() {
                    text.push_str(&delta);
                }
            }
        }
        assert!(buffer.is_empty());
        assert_eq!(text, "Привет 🦀");
        let (final_chunk, _) = decoder.finish().unwrap();
        assert!(final_chunk.choices[0].delta.content.is_none());
        assert_eq!(
            final_chunk.choices[0].finish_reason.as_deref(),
            Some("stop")
        );
    }
}

#[test]
fn direct_protocol_sse_rejects_missing_terminals_indexes_and_changed_snapshots() {
    for api_type in new_protocols() {
        let events = protocol_sse_values(api_type, "hello", &[]);
        let mut decoder = protocols::ProtocolStream::new(api_type).unwrap();
        for event in &events[..events.len() - 1] {
            decoder
                .apply_event(&sse_wire(std::slice::from_ref(event)))
                .unwrap();
        }
        assert!(decoder.finish().is_err());
        for malformed in [
            json!({"type": "error", "error": {"message": "private provider payload"}}),
            if api_type == CustomApiType::OpenAiResponses {
                json!({"type": "response.output_text.delta", "output_index": 99, "content_index": 0, "delta": "private provider payload"})
            } else {
                json!({"type": "content_block_delta", "index": 99, "delta": {"type": "text_delta", "text": "private provider payload"}})
            },
        ] {
            let mut decoder = protocols::ProtocolStream::new(api_type).unwrap();
            decoder.apply_event(&sse_wire(&events[..1])).unwrap();
            let error = decoder
                .apply_event(&sse_wire(&[malformed]))
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private provider payload"));
        }
    }
    let mut events = protocol_sse_values(CustomApiType::OpenAiResponses, "hello", &[]);
    events.last_mut().unwrap()["response"]["output"][0]["content"][0]["text"] = json!("different");
    let mut decoder = protocols::ProtocolStream::new(CustomApiType::OpenAiResponses).unwrap();
    for event in &events[..events.len() - 1] {
        decoder
            .apply_event(&sse_wire(std::slice::from_ref(event)))
            .unwrap();
    }
    assert!(
        decoder
            .apply_event(&sse_wire(&events[events.len() - 1..]))
            .is_err()
    );
}

#[tokio::test]
async fn direct_protocol_failed_or_truncated_stream_never_dispatches_tools() {
    let calls = [call(
        "shell",
        "run_shell_command",
        json!({"command": "pwd"}),
    )];
    for api_type in new_protocols() {
        for terminal_error in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let mut events = protocol_sse_values(api_type, "", &calls);
            events.pop();
            if terminal_error {
                events.push(
                    json!({"type": "error", "error": {"message": "private provider payload"}}),
                );
            }
            let _mock = server
                .mock("POST", protocol_endpoint(api_type))
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body(sse_wire(&events))
                .create_async()
                .await;
            let events = generate(
                protocol_route(api_type, format!("{}/v1", server.url())),
                super::super::RequestParams::new_for_test(),
                vec![api::ToolType::RunShellCommand],
            )
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
            assert!(events.iter().any(Result::is_err));
            assert!(
                events
                    .iter()
                    .filter_map(|event| event.as_ref().err())
                    .all(|error| !error.to_string().contains("private provider payload"))
            );
            let (_, dispatched, finished) =
                extract_output(events.into_iter().filter_map(Result::ok).collect());
            assert!(dispatched.is_empty());
            assert_eq!(finished, 0);
        }
    }
}

#[test]
fn direct_protocol_json_rejects_incomplete_truncated_duplicate_and_malformed_tools() {
    for api_type in new_protocols() {
        let duplicate = call("same", "run_shell_command", json!({"command": "pwd"}));
        assert!(
            protocols::decode_response(
                api_type,
                protocol_response(api_type, "", &[duplicate.clone(), duplicate])
            )
            .is_err()
        );
        let mut incomplete = protocol_response(api_type, "partial", &[]);
        if api_type == CustomApiType::OpenAiResponses {
            incomplete["status"] = json!("incomplete");
        } else {
            incomplete["stop_reason"] = json!("max_tokens");
        }
        assert!(protocols::decode_response(api_type, incomplete).is_err());
        let mut invalid_tool = protocol_response(
            api_type,
            "",
            &[call("id", "run_shell_command", json!({"command": "pwd"}))],
        );
        if api_type == CustomApiType::OpenAiResponses {
            invalid_tool["output"][0]["arguments"] = json!("private malformed args");
        } else {
            invalid_tool["content"][0]["input"] = json!("private malformed args");
        }
        let error = protocols::decode_response(api_type, invalid_tool)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private malformed args"));
    }
}

#[tokio::test]
async fn direct_protocol_http_errors_are_sanitized_and_context_overflow_remains_retryable() {
    for api_type in new_protocols() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server.mock("POST", protocol_endpoint(api_type)).with_status(400)
            .with_body(json!({"error": {"message": "context token limit exceeded: private prompt and credentials"}}).to_string()).create_async().await;
        let error = complete_text(
            protocol_route(api_type, format!("{}/v1", server.url())),
            "system".into(),
            "user".into(),
        )
        .await
        .unwrap_err();
        assert!(is_context_overflow_error(&error));
        assert!(!error.to_string().contains("private prompt"));
    }
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/responses")
        .with_status(400)
        .with_body("unsupported prompt_cache_options: private prompt")
        .create_async()
        .await;
    let mut route = protocol_route(
        CustomApiType::OpenAiResponses,
        format!("{}/v1", server.url()),
    );
    route.prompt_caching = false;
    let error = complete_text(route, "system".into(), "user".into())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("explicit"));
    assert!(!error.contains("private prompt"));
}

#[tokio::test]
async fn direct_protocol_anthropic_model_discovery_authenticates_and_follows_pages() {
    let mut server = mockito::Server::new_async().await;
    let first = server
        .mock("GET", "/v1/models")
        .match_query(Matcher::Missing)
        .match_header("x-api-key", "synthetic-credential-fixture")
        .match_header("anthropic-version", "2023-06-01")
        .match_header("authorization", Matcher::Missing)
        .with_status(200)
        .with_body(
            json!({"data": [{"id": "one"}], "has_more": true, "last_id": "cursor-1"}).to_string(),
        )
        .create_async()
        .await;
    let second = server
        .mock("GET", "/v1/models")
        .match_query(Matcher::UrlEncoded("after_id".into(), "cursor-1".into()))
        .match_header("x-api-key", "synthetic-credential-fixture")
        .with_status(200)
        .with_body(
            json!({"data": [{"id": "one"}, {"id": " two "}], "has_more": false, "last_id": "two"})
                .to_string(),
        )
        .create_async()
        .await;
    let models = fetch_models_for_protocol(
        &format!("{}/v1", server.url()),
        Some("synthetic-credential-fixture"),
        CustomApiType::AnthropicMessages,
    )
    .await
    .unwrap();
    assert_eq!(models, ["one", "two"]);
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn direct_protocol_anthropic_discovery_rejects_repeated_pagination_cursor() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/v1/models")
        .match_query(Matcher::Any)
        .with_status(200)
        .with_body(
            json!({"data": [{"id": "one"}], "has_more": true, "last_id": "cursor"}).to_string(),
        )
        .expect(2)
        .create_async()
        .await;
    assert!(
        fetch_models_for_protocol(
            &format!("{}/v1", server.url()),
            None,
            CustomApiType::AnthropicMessages
        )
        .await
        .is_err()
    );
}

fn stored_messages(events: Vec<api::ResponseEvent>) -> Vec<api::Message> {
    let mut messages: Vec<api::Message> = Vec::new();
    for event in events {
        let Some(api::response_event::Type::ClientActions(actions)) = event.r#type else {
            continue;
        };
        for action in actions.actions {
            match action.action {
                Some(api::client_action::Action::AddMessagesToTask(add)) => {
                    messages.extend(add.messages)
                }
                Some(api::client_action::Action::AppendToMessageContent(append)) => {
                    let incoming = append.message.unwrap();
                    let previous = messages
                        .iter_mut()
                        .find(|message| message.id == incoming.id)
                        .unwrap();
                    let Some(api::message::Message::AgentOutput(previous)) =
                        previous.message.as_mut()
                    else {
                        panic!("expected output")
                    };
                    let Some(api::message::Message::AgentOutput(incoming)) = incoming.message
                    else {
                        panic!("expected delta")
                    };
                    previous.text.push_str(&incoming.text);
                }
                Some(api::client_action::Action::UpdateTaskMessage(update)) => {
                    assert_eq!(update.mask.unwrap().paths, ["server_message_data"]);
                    let incoming = update.message.unwrap();
                    messages
                        .iter_mut()
                        .find(|message| message.id == incoming.id)
                        .unwrap()
                        .server_message_data = incoming.server_message_data;
                }
                _ => {}
            }
        }
    }
    messages
}

#[tokio::test]
async fn direct_protocol_responses_continuation_preserves_phase_and_bound_reasoning_items() {
    let api_type = CustomApiType::OpenAiResponses;
    let calls = [call(
        "shell",
        "run_shell_command",
        json!({"command": "pwd"}),
    )];
    let reasoning = json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "synthetic-opaque-reasoning"});
    for streaming in [false, true] {
        let mut server = mockito::Server::new_async().await;
        let route = protocol_route(api_type, format!("{}/v1", server.url()));
        let mut response = protocol_response(api_type, "Checking", &calls);
        response["output"][0]["phase"] = json!("commentary");
        response["output"]
            .as_array_mut()
            .unwrap()
            .insert(0, reasoning.clone());
        let body = if streaming {
            let mut events = protocol_sse_values(api_type, "Checking", &calls);
            for event in &mut events {
                if let Some(index) = event["output_index"].as_u64() {
                    event["output_index"] = json!(index + 1);
                }
                if event["item"]["type"] == "message" {
                    event["item"]["phase"] = json!("commentary");
                }
            }
            events.last_mut().unwrap()["response"] = response.clone();
            events.insert(
                1,
                json!({"type": "response.output_item.added", "output_index": 0, "item": reasoning}),
            );
            events.insert(
                2,
                json!({"type": "response.output_item.done", "output_index": 0, "item": reasoning}),
            );
            sse_wire(&events)
        } else {
            response.to_string()
        };
        let initial = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::PartialJson(
                json!({"include": ["reasoning.encrypted_content"], "store": false}),
            ))
            .with_status(200)
            .with_header(
                "content-type",
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let params = super::super::RequestParams::new_for_test();
        let events = generate(route.clone(), params, vec![api::ToolType::RunShellCommand])
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        initial.assert_async().await;
        initial.remove_async().await;
        let mut messages = stored_messages(events);
        messages.push(api::Message {
            id: "result".to_string(),
            task_id: "root".to_string(),
            message: Some(api::message::Message::ToolCallResult(
                api::message::ToolCallResult {
                    tool_call_id: "shell".to_string(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        });
        let history = openai_messages_from_api_messages_with_tool_policy_and_vision(
            &messages,
            false,
            MAX_CONTEXT_CHARS,
            true,
        )
        .unwrap();
        let request = protocol_request(history.clone());
        let wire = protocols::request_body(&route, &request).unwrap();
        let input = wire["input"].as_array().unwrap();
        assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "function_call")
                .count(),
            1
        );
        assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "function_call_output")
                .count(),
            1
        );
        assert_eq!(
            input
                .iter()
                .find(|item| item["type"] == "reasoning")
                .unwrap(),
            &reasoning
        );
        assert_eq!(
            input.iter().find(|item| item["type"] == "message").unwrap()["phase"],
            "commentary"
        );
        let follow_up = server
            .mock("POST", "/v1/responses")
            .match_body(Matcher::Json(wire))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(protocol_response(api_type, "Done", &[]).to_string())
            .create_async()
            .await;
        let events =
            stream_chat_completion(route.clone(), request, "root".into(), "next".into(), vec![])
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
        assert_eq!(extract_output(events).0, "Done");
        follow_up.assert_async().await;

        for changed_field in ["provider", "endpoint", "model", "protocol"] {
            let mut switched = route.clone();
            match changed_field {
                "provider" => switched.provider_name = "other".into(),
                "endpoint" => switched.base_url = "http://localhost:1234/v1".into(),
                "protocol" => switched.api_type = CustomApiType::AnthropicMessages,
                _ => switched.model = "different-model".into(),
            }
            let request = protocol_request(history.clone());
            let switched_wire = protocols::request_body(&switched, &request).unwrap();
            assert!(
                !switched_wire
                    .to_string()
                    .contains("synthetic-opaque-reasoning")
            );
            if switched.api_type == CustomApiType::AnthropicMessages {
                assert!(switched_wire.to_string().contains("tool_use"));
            } else {
                assert!(
                    switched_wire["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["type"] == "function_call")
                );
            }
        }
        let orphaned = history.into_iter().filter(|message| !matches!(message.content, Some(ChatMessageContent::Text(ref text)) if text == "Checking")).collect();
        let orphan_wire = protocols::request_body(&route, &protocol_request(orphaned)).unwrap();
        assert!(
            orphan_wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "function_call")
        );
    }
}

fn anthropic_thinking_response(text: &str) -> Value {
    let calls = [
        call("shell", "run_shell_command", json!({"command": "pwd"})),
        call(
            "mcp",
            "call_mcp_tool",
            json!({"name": "inspect", "server_id": "local", "args": {}}),
        ),
    ];
    let mut response = protocol_response(CustomApiType::AnthropicMessages, text, &calls);
    let content = response["content"].as_array_mut().unwrap();
    content.insert(0, json!({"type": "thinking", "thinking": "Private thought fixture 🦀", "signature": "synthetic-signature-one"}));
    content.insert(
        1,
        json!({"type": "thinking", "thinking": "", "signature": "synthetic-signature-omitted"}),
    );
    content.insert(
        2,
        json!({"type": "redacted_thinking", "data": "synthetic-redacted-data"}),
    );
    response
}

fn anthropic_thinking_sse(response: &Value) -> String {
    let mut events = vec![json!({"type": "message_start", "message": {
        "id": response["id"], "type": "message", "role": "assistant", "content": []}})];
    for (index, block) in response["content"].as_array().unwrap().iter().enumerate() {
        let mut initial = block.clone();
        match block["type"].as_str().unwrap() {
            "thinking" => {
                initial["thinking"] = json!("");
                initial["signature"] = json!("");
            }
            "text" => initial["text"] = json!(""),
            "tool_use" => initial["input"] = json!({}),
            _ => {}
        }
        events
            .push(json!({"type": "content_block_start", "index": index, "content_block": initial}));
        match block["type"].as_str().unwrap() {
            "thinking" => {
                for text in block["thinking"].as_str().unwrap().chars() {
                    events.push(json!({"type": "content_block_delta", "index": index,
                        "delta": {"type": "thinking_delta", "thinking": text.to_string()}}));
                }
                for fragment in block["signature"].as_str().unwrap().as_bytes().chunks(4) {
                    events.push(json!({"type": "content_block_delta", "index": index,
                        "delta": {"type": "signature_delta", "signature": std::str::from_utf8(fragment).unwrap()}}));
                }
            }
            "text" => {
                for text in block["text"].as_str().unwrap().chars() {
                    events.push(json!({"type": "content_block_delta", "index": index,
                    "delta": {"type": "text_delta", "text": text.to_string()}}));
                }
            }
            "tool_use" => {
                for fragment in block["input"].to_string().chars() {
                    events.push(json!({"type": "content_block_delta", "index": index,
                    "delta": {"type": "input_json_delta", "partial_json": fragment.to_string()}}));
                }
            }
            _ => {}
        }
        events.push(json!({"type": "content_block_stop", "index": index}));
    }
    events
        .push(json!({"type": "message_delta", "delta": {"stop_reason": response["stop_reason"]}}));
    events.push(json!({"type": "message_stop"}));
    sse_wire(&events)
}

#[tokio::test]
async fn direct_protocol_anthropic_continuation_preserves_complete_signed_content() {
    let api_type = CustomApiType::AnthropicMessages;
    for streaming in [false, true] {
        for text in ["Checking", ""] {
            let mut server = mockito::Server::new_async().await;
            let route = protocol_route(api_type, format!("{}/v1", server.url()));
            let response = anthropic_thinking_response(text);
            let prefix = vec![
                ChatMessage::system("Stable system".into()),
                ChatMessage::user("Inspect the workspace".into()),
            ];
            let initial = server
                .mock("POST", "/v1/messages")
                .with_status(200)
                .with_header(
                    "content-type",
                    if streaming {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                )
                .with_body(if streaming {
                    anthropic_thinking_sse(&response)
                } else {
                    response.to_string()
                })
                .expect(1)
                .create_async()
                .await;
            let events = stream_chat_completion_with_tool_policy(
                route.clone(),
                protocol_request(prefix.clone()),
                "root".into(),
                "first".into(),
                vec![],
                vec![api::ToolType::RunShellCommand, api::ToolType::CallMcpTool],
                false,
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
            initial.assert_async().await;
            initial.remove_async().await;
            let visible = extract_output(events.clone());
            assert_eq!(visible.0, text);
            assert_eq!(visible.1.len(), 2);
            let mut stored = stored_messages(events);
            for id in ["shell", "mcp"] {
                stored.push(api::Message {
                    id: format!("result-{id}"),
                    task_id: "root".into(),
                    message: Some(api::message::Message::ToolCallResult(
                        api::message::ToolCallResult {
                            tool_call_id: id.into(),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                });
            }
            let mut history = prefix;
            history.extend(
                openai_messages_from_api_messages_with_tool_policy_and_vision(
                    &stored,
                    false,
                    MAX_CONTEXT_CHARS,
                    true,
                )
                .unwrap(),
            );
            let request = protocol_request(history.clone());
            let wire = protocols::request_body(&route, &request).unwrap();
            assert_eq!(wire["messages"][1]["content"], response["content"]);
            assert_eq!(wire["messages"][2]["content"].as_array().unwrap().len(), 2);
            assert_eq!(wire["messages"][2]["content"][0]["tool_use_id"], "shell");
            assert_eq!(wire["messages"][2]["content"][1]["tool_use_id"], "mcp");
            let mut cache_changed = route.clone();
            cache_changed.prompt_caching = false;
            assert_eq!(
                protocols::request_body(&cache_changed, &request).unwrap()["messages"][1]["content"],
                response["content"]
            );
            let follow_up = server
                .mock("POST", "/v1/messages")
                .match_body(Matcher::Json(wire))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(protocol_response(api_type, "Done", &[]).to_string())
                .create_async()
                .await;
            let events = stream_chat_completion(
                route.clone(),
                request,
                "root".into(),
                "next".into(),
                vec![],
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
            assert_eq!(extract_output(events).0, "Done");
            follow_up.assert_async().await;

            for change in [
                "protocol",
                "provider",
                "endpoint",
                "model",
                "system",
                "tools",
                "earlier message",
                "compaction",
            ] {
                let mut changed_route = route.clone();
                let mut changed_request = protocol_request(history.clone());
                match change {
                    "protocol" => changed_route.api_type = CustomApiType::OpenAiResponses,
                    "provider" => changed_route.provider_name = "other".into(),
                    "endpoint" => changed_route.base_url = "http://localhost:1234/v1".into(),
                    "model" => changed_route.model = "other-model".into(),
                    "system" => {
                        changed_request.messages[0] =
                            ChatMessage::system("Changed current directory or MCP context".into())
                    }
                    "tools" => changed_request.tools.clear(),
                    "earlier message" => {
                        changed_request.messages[1] =
                            ChatMessage::user("Edited initial question".into())
                    }
                    _ => {
                        changed_request.messages.remove(1);
                    }
                }
                let changed = protocols::request_body(&changed_route, &changed_request)
                    .unwrap()
                    .to_string();
                assert!(!changed.contains("synthetic-signature"), "{change}");
                assert!(!changed.contains("synthetic-redacted-data"), "{change}");
                assert!(!changed.contains("Private thought fixture"), "{change}");
                assert!(changed.contains("run_shell_command"), "{change}");
                assert!(
                    changed.contains("result-shell")
                        || changed.contains("tool_use_id")
                        || changed.contains("function_call_output"),
                    "{change}"
                );
            }
        }
    }
}

#[test]
fn direct_protocol_anthropic_prefix_change_keeps_old_thinking_retired_after_restore() {
    let api_type = CustomApiType::AnthropicMessages;
    let route = protocol_route(api_type, "http://localhost:1234/v1".into());
    let prefix = vec![
        ChatMessage::system("Original system".into()),
        ChatMessage::user("Question".into()),
    ];
    let mut first =
        protocols::decode_response(api_type, anthropic_thinking_response("Checking")).unwrap();
    protocols::bind_output_to_request(
        &route,
        &protocol_request(prefix.clone()),
        &mut first.response_output,
    )
    .unwrap();
    let events = events_from_non_streaming_response(
        first,
        &route,
        "root",
        "first",
        vec![],
        &[api::ToolType::RunShellCommand, api::ToolType::CallMcpTool],
        false,
    )
    .unwrap();
    let stored = stored_messages(events);
    let mut history = prefix;
    history.extend(
        openai_messages_from_api_messages_with_tool_policy_and_vision(
            &stored,
            false,
            MAX_CONTEXT_CHARS,
            true,
        )
        .unwrap(),
    );
    history.push(ChatMessage::tool("shell".into(), "shell result".into()));
    history.push(ChatMessage::tool("mcp".into(), "mcp result".into()));
    let mut changed = protocol_request(history.clone());
    changed.messages[0] = ChatMessage::system("Changed system".into());
    let mut final_value = protocol_response(api_type, "Done after edit", &[]);
    final_value["id"] = json!("after-prefix-edit");
    let mut completed = protocols::decode_response(api_type, final_value).unwrap();
    protocols::bind_output_to_request(&route, &changed, &mut completed.response_output).unwrap();
    let events = events_from_non_streaming_response(
        completed,
        &route,
        "root",
        "after-edit",
        vec![],
        &[],
        false,
    )
    .unwrap();
    let after_edit = stored_messages(events);
    history.extend(
        openai_messages_from_api_messages_with_tool_policy_and_vision(
            &after_edit,
            false,
            MAX_CONTEXT_CHARS,
            true,
        )
        .unwrap(),
    );
    let restored = protocols::request_body(&route, &protocol_request(history)).unwrap();
    assert!(!restored.to_string().contains("synthetic-signature"));
    assert!(!restored.to_string().contains("synthetic-redacted-data"));
    assert!(restored.to_string().contains("Done after edit"));
}

#[test]
fn direct_protocol_anthropic_rejects_missing_signatures_without_visible_thinking() {
    for field in ["signature", "data"] {
        let mut response = anthropic_thinking_response("Checking");
        let index = if field == "signature" { 0 } else { 2 };
        response["content"][index]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let error = protocols::decode_response(CustomApiType::AnthropicMessages, response)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("Private thought fixture"));
    }
}

#[tokio::test]
async fn direct_protocol_anthropic_signature_error_is_actionable_and_redacted() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server.mock("POST", "/v1/messages").with_status(400)
        .with_body("Invalid signature in thinking block: bound to a different conversation; private payload")
        .create_async().await;
    let error = complete_text(
        protocol_route(
            CustomApiType::AnthropicMessages,
            format!("{}/v1", server.url()),
        ),
        "system".into(),
        "user".into(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("preserved thinking"));
    assert!(!error.contains("private payload"));
}
