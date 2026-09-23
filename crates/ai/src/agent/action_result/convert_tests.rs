use super::*;

#[test]
fn local_mcp_resource_contents_convert_to_agent_results() {
    let text = convert_mcp_resource_content(rmcp::model::ResourceContents::text(
        "local file contents",
        "file:///tmp/readme.txt",
    ))
    .expect("supported text resource should be converted");
    let Some(api::mcp_resource_content::ContentType::Text(text)) = text.content_type else {
        panic!("text MCP resource should remain text in the agent result");
    };
    assert_eq!(text.content, "local file contents");
    assert_eq!(text.mime_type, "text/plain");

    let binary =
        convert_mcp_resource_content(rmcp::model::ResourceContents::BlobResourceContents {
            uri: "file:///tmp/image.png".to_string(),
            mime_type: Some("image/png".to_string()),
            blob: "local image bytes".to_string(),
            meta: None,
        })
        .expect("supported binary resource should be converted");
    let Some(api::mcp_resource_content::ContentType::Binary(binary)) = binary.content_type else {
        panic!("binary MCP resource should remain binary in the agent result");
    };
    assert_eq!(binary.mime_type, "image/png");
    assert_eq!(binary.data.as_slice(), b"local image bytes");
}

#[test]
fn local_mcp_tool_text_and_resource_results_survive_conversion() {
    let result = convert_mcp_tool_call_result(rmcp::model::CallToolResult::success(vec![
        rmcp::model::ContentBlock::text("local tool output"),
        rmcp::model::ContentBlock::resource(rmcp::model::ResourceContents::text(
            "local resource contents",
            "file:///tmp/local-resource.txt",
        )),
    ]));
    let api::call_mcp_tool_result::Result::Success(success) = result else {
        panic!("successful local MCP tool result should stay successful");
    };

    assert_eq!(success.results.len(), 2);
    assert!(matches!(
        &success.results[0].result,
        Some(api::call_mcp_tool_result::success::result::Result::Text(text))
            if text.text == "local tool output"
    ));
    let Some(api::call_mcp_tool_result::success::result::Result::Resource(resource)) =
        &success.results[1].result
    else {
        panic!("embedded local MCP resource should stay a resource");
    };
    assert_eq!(resource.uri, "file:///tmp/local-resource.txt");
    assert!(matches!(
        &resource.content_type,
        Some(api::mcp_resource_content::ContentType::Text(text))
            if text.content == "local resource contents"
    ));
}

#[test]
fn ask_user_question_skipped_by_auto_approve_converts_to_skipped_answers() {
    let result = api::request::input::tool_call_result::Result::from(
        AskUserQuestionResult::SkippedByAutoApprove {
            question_ids: vec!["q1".to_string(), "q2".to_string()],
        },
    );

    let api::request::input::tool_call_result::Result::AskUserQuestion(result) = result else {
        panic!("expected ask_user_question result");
    };

    let Some(api::ask_user_question_result::Result::Success(success)) = result.result else {
        panic!("expected success result");
    };

    assert_eq!(success.answers.len(), 2);
    assert_eq!(success.answers[0].question_id, "q1");
    assert_eq!(success.answers[1].question_id, "q2");
    assert!(matches!(
        success.answers[0].answer,
        Some(AskUserQuestionAnswer::Skipped(()))
    ));
    assert!(matches!(
        success.answers[1].answer,
        Some(AskUserQuestionAnswer::Skipped(()))
    ));
}
