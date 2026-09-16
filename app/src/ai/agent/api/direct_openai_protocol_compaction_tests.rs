use super::*;
use mockito::Matcher;

fn history() -> api::Task {
    let messages = (0..12)
        .map(|index| {
            let text = if index < 8 {
                "history ".repeat(225)
            } else {
                "recent".to_string()
            };
            let payload = if index % 2 == 0 {
                api::message::Message::UserQuery(api::message::UserQuery {
                    query: text,
                    context: None,
                    referenced_attachments: Default::default(),
                    mode: None,
                    intended_agent: 0,
                })
            } else {
                api::message::Message::AgentOutput(api::message::AgentOutput { text })
            };
            api::Message {
                id: format!("history-message-{index}"),
                task_id: "root".to_string(),
                request_id: format!("request-{}", index / 2),
                message: Some(payload),
                ..Default::default()
            }
        })
        .collect();
    api::Task {
        id: "root".to_string(),
        messages,
        ..Default::default()
    }
}

fn summary(snapshot: &LocalCompactionSnapshot) -> String {
    json!({
        "schema_version": 1,
        "first_message_id": snapshot.first_message_id(),
        "last_message_id": snapshot.last_message_id(),
        "message_count": snapshot.message_count(),
        "range_checksum": snapshot.range_checksum(),
        "goals": ["keep local decisions"],
        "user_constraints": [],
        "decisions": [],
        "files_symbols": [],
        "commands_outcomes": [],
        "unresolved_work": [],
        "child_agent_results": [],
        "narrative": "A bounded summary from the selected protocol."
    })
    .to_string()
}

#[tokio::test]
async fn direct_openai_protocol_compaction_retries_overflow_with_smaller_history() {
    for api_type in [
        CustomApiType::OpenAiResponses,
        CustomApiType::AnthropicMessages,
    ] {
        let mut server = mockito::Server::new_async().await;
        let route = CustomProviderRoute {
            provider_name: "local".to_string(),
            base_url: format!("{}/v1", server.url()),
            model: "model".to_string(),
            api_key: None,
            api_type,
            prompt_caching: true,
            capabilities: CustomProviderCapabilities {
                context_window_tokens: Some(8_000),
                ..Default::default()
            },
        };
        let task = history();
        let identity = LocalCompactionRoute {
            model_id: "custom/local/model".to_string(),
            provider_name: route.provider_name.clone(),
            model: route.model.clone(),
            configuration_fingerprint: route_configuration_fingerprint(
                &route.provider_name,
                &route.base_url,
                &route.model,
                &route.capabilities,
                route.api_type,
                route.prompt_caching,
            ),
        };
        let limits = LocalCompactionLimits::for_context_budget(route.context_char_budget());
        let capture = |limits| {
            LocalCompactionSnapshot::capture(
                "11111111-1111-4111-8111-111111111111",
                std::slice::from_ref(&task),
                "root",
                identity.clone(),
                limits,
            )
            .unwrap()
        };
        let initial = capture(limits);
        let retry = capture(limits.retry_after_context_overflow());
        assert!(retry.message_count() < initial.message_count());
        let summary = summary(&retry);
        let (path, error, success) = match api_type {
            CustomApiType::OpenAiResponses => (
                "/v1/responses",
                json!({"error":{"code":"context_length_exceeded", "message":"context token limit exceeded"}}),
                json!({"id":"response-summary", "status":"completed", "output":[{
                    "id":"message-summary", "type":"message", "role":"assistant", "status":"completed",
                    "content":[{"type":"output_text", "text":summary}]
                }]}),
            ),
            CustomApiType::AnthropicMessages => (
                "/v1/messages",
                json!({"type":"error", "error":{"type":"invalid_request_error", "message":"prompt is too long: 22000 tokens > 16000 maximum"}}),
                json!({"id":"message-summary", "type":"message", "role":"assistant", "stop_reason":"end_turn",
                    "content":[{"type":"text", "text":summary}]}),
            ),
            CustomApiType::OpenAiCompatible => unreachable!(),
        };
        let overflow = server
            .mock("POST", path)
            .match_body(Matcher::Regex(format!(
                "last_message_id.*{}",
                initial.last_message_id()
            )))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(error.to_string())
            .expect(1)
            .create_async()
            .await;
        let completed = server
            .mock("POST", path)
            .match_body(Matcher::AllOf(vec![
                Matcher::Regex(format!("last_message_id.*{}", retry.last_message_id())),
                Matcher::PartialJson(json!({"stream":false, "model":"model"})),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(success.to_string())
            .expect(1)
            .create_async()
            .await;
        let mut params = crate::ai::agent::api::RequestParams::new_for_test();
        params.conversation_id = "11111111-1111-4111-8111-111111111111".to_string();
        params.model = "custom/local/model".into();
        params.request_task_id = Some("root".to_string());
        params.tasks = vec![task];
        params.input = vec![AIAgentInput::SummarizeConversation {
            prompt: None,
            context: Arc::from([]),
        }];
        let events = generate(route, params, vec![])
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        let events = events
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("protocol compaction should recover");
        assert_eq!(events.len(), 3);
        let Some(api::response_event::Type::ClientActions(actions)) = &events[1].r#type else {
            panic!("compaction transaction missing")
        };
        assert_eq!(actions.actions.len(), 1);
        assert!(matches!(
            actions.actions[0].action,
            Some(api::client_action::Action::MoveMessagesToNewTask(_))
        ));
        overflow.assert_async().await;
        completed.assert_async().await;
    }
}
