use super::{AIClient, AgentRunEvent, Artifact};
use crate::ai::agent::api::direct_openai::CustomProviderRoute;
use crate::ai::generate_code_review_content::api::{GenerateCodeReviewContentRequest, OutputType};
use crate::ai::predict::predict_am_queries::PredictAMQueriesRequest;
use crate::notebooks::NotebookId;
use crate::server::server_api::ServerApi;
use crate::workflows::workflow::Workflow;

fn local_provider_route(base_url: String) -> CustomProviderRoute {
    CustomProviderRoute {
        provider_name: "local".to_string(),
        base_url,
        model: "test-model".to_string(),
        api_key: Some("test-key".to_string()),
    }
}

#[tokio::test]
async fn generate_code_review_content_uses_local_openai_compatible_provider() {
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
                        "message": { "content": "Tighten local provider routing" },
                        "finish_reason": "stop"
                    }
                ]
            }"#,
        )
        .create_async()
        .await;
    let server_api = ServerApi::new_for_test_with_local_ai_route(local_provider_route(format!(
        "{}/v1",
        server.url()
    )));

    let response = server_api
        .generate_code_review_content(GenerateCodeReviewContentRequest {
            output_type: OutputType::CommitMessage,
            diff: "diff --git a/app.rs b/app.rs".to_string(),
            branch_name: "local-ai".to_string(),
            commit_messages: vec![],
        })
        .await
        .unwrap();

    assert_eq!(response.content, "Tighten local provider routing");
}

#[tokio::test]
async fn generate_metadata_for_command_uses_local_openai_compatible_provider() {
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
                            "content": "{\"command\":\"git status --short\",\"title\":\"Show Git Status\",\"description\":\"List changed files.\",\"arguments\":[]}"
                        },
                        "finish_reason": "stop"
                    }
                ]
            }"#,
        )
        .create_async()
        .await;
    let server_api = ServerApi::new_for_test_with_local_ai_route(local_provider_route(format!(
        "{}/v1",
        server.url()
    )));

    let response = server_api
        .generate_metadata_for_command("git status --short".to_string())
        .await
        .unwrap();

    assert_eq!(response.command, "git status --short");
    assert_eq!(response.title, "Show Git Status");
}

#[tokio::test]
async fn generate_commands_from_natural_language_uses_local_openai_compatible_provider() {
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
                            "content": "{\"commands\":[{\"command\":\"ls -la\",\"description\":\"List files with details.\",\"parameters\":[]}]}"
                        },
                        "finish_reason": "stop"
                    }
                ]
            }"#,
        )
        .create_async()
        .await;
    let server_api = ServerApi::new_for_test_with_local_ai_route(local_provider_route(format!(
        "{}/v1",
        server.url()
    )));

    let commands = server_api
        .generate_commands_from_natural_language("list files".to_string(), None)
        .await
        .unwrap();
    let workflow = Workflow::from(commands.into_iter().next().unwrap());

    assert_eq!(workflow.command(), Some("ls -la"));
    assert_eq!(workflow.name(), "List files with details.");
}

#[tokio::test]
async fn predict_am_queries_uses_local_openai_compatible_provider() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
                "choices": [
                    {
                        "message": { "content": "{\"suggestion\":\"explain the failed test\"}" },
                        "finish_reason": "stop"
                    }
                ]
            }"#,
        )
        .create_async()
        .await;
    let server_api = ServerApi::new_for_test_with_local_ai_route(local_provider_route(format!(
        "{}/v1",
        server.url()
    )));

    let response = server_api
        .predict_am_queries(&PredictAMQueriesRequest {
            context_messages: vec!["cargo test failed".to_string()],
            partial_query: "explain".to_string(),
            system_context: None,
        })
        .await
        .unwrap();

    assert_eq!(response.suggestion, "explain the failed test");
}

#[test]
fn test_deserialize_plan_artifact() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN",
        "data": {
            "document_uid": "doc-uid-123",
            "notebook_uid": "1234567890123456789012",
            "title": "My Plan"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::Plan {
        document_uid,
        notebook_uid,
        title,
    } = &artifact
    else {
        panic!("expected Plan artifact");
    };
    assert_eq!(document_uid, "doc-uid-123");
    assert_eq!(
        notebook_uid.as_ref().map(|n| n.to_string()),
        Some("1234567890123456789012".to_string())
    );
    assert_eq!(*title, Some("My Plan".to_string()));
}

#[test]
fn test_deserialize_pull_request_artifact() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://github.com/org/repo/pull/42",
            "branch": "feature-branch"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::PullRequest {
        url,
        branch,
        repo,
        number,
    } = &artifact
    else {
        panic!("expected PullRequest artifact");
    };
    assert_eq!(url, "https://github.com/org/repo/pull/42");
    assert_eq!(branch, "feature-branch");
    assert_eq!(*repo, Some("repo".to_string()));
    assert_eq!(*number, Some(42));
}

#[test]
fn test_deserialize_pull_request_non_github_url() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://gitlab.com/org/repo/merge_requests/42",
            "branch": "feature-branch"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::PullRequest { repo, number, .. } = &artifact else {
        panic!("expected PullRequest artifact");
    };
    assert_eq!(*repo, None);
    assert_eq!(*number, None);
}

#[test]
fn test_deserialize_plan_artifact_with_optional_fields_missing() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN",
        "data": {
            "document_uid": "doc-uid-123",
            "notebook_uid": "abcdefghijklmnopqrstuv"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::Plan {
        document_uid,
        notebook_uid,
        title,
    } = &artifact
    else {
        panic!("expected Plan artifact");
    };
    assert_eq!(document_uid, "doc-uid-123");
    assert_eq!(
        notebook_uid.as_ref().map(|n| n.to_string()),
        Some("abcdefghijklmnopqrstuv".to_string())
    );
    assert!(title.is_none());
}

#[test]
fn test_deserialize_artifact_missing_data_field() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN"
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_invalid_plan_data() {
    // Missing required `document_uid` field should fail deserialization
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN",
        "data": {
            "title": "Only title, no document_uid"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_invalid_pr_data() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://github.com/org/repo/pull/1"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_unknown_variant() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "UNKNOWN_TYPE",
        "data": {
            "some_field": "value"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("unknown variant"));
}

// ---------------------------------------------------------------------------------------------------------------------
//  We test roundtripping serialize and deserialize since we use this for persisting artifacts for local conversations.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn test_artifact_plan_serialize_deserialize_roundtrip() {
    let original = Artifact::Plan {
        document_uid: "doc-123".to_string(),
        notebook_uid: Some(NotebookId::from("notebook12345678901234".to_string())),
        title: Some("My Plan".to_string()),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn test_deserialize_agent_run_events_with_optional_fields() {
    let json = r#"[
        {
            "event_type": "run_started",
            "run_id": "run-1",
            "ref_id": null,
            "execution_id": "exec-1",
            "occurred_at": "2026-04-09T20:00:00Z",
            "sequence": 7
        },
        {
            "event_type": "new_message",
            "run_id": "run-2",
            "ref_id": "message-9",
            "execution_id": null,
            "occurred_at": "2026-04-09T20:05:00Z",
            "sequence": 8
        }
    ]"#;

    let events: Vec<AgentRunEvent> = serde_json::from_str(json).unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "run_started");
    assert_eq!(events[0].execution_id.as_deref(), Some("exec-1"));
    assert_eq!(events[0].ref_id, None);
    assert_eq!(events[0].sequence, 7);
    assert_eq!(events[1].event_type, "new_message");
    assert_eq!(events[1].ref_id.as_deref(), Some("message-9"));
    assert_eq!(events[1].execution_id, None);
    assert_eq!(events[1].sequence, 8);
}

#[test]
fn test_artifact_plan_serialize_deserialize_roundtrip_no_notebook_uid() {
    let original = Artifact::Plan {
        document_uid: "doc-123".to_string(),
        notebook_uid: None,
        title: Some("My Plan".to_string()),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_pr_serialize_deserialize_roundtrip() {
    let original = Artifact::PullRequest {
        url: "https://github.com/org/repo/pull/42".to_string(),
        branch: "feature-branch".to_string(),
        repo: Some("repo".to_string()),
        number: Some(42),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    // repo/number are re-derived from URL on deserialize, so should match
    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_file_serialize_deserialize_roundtrip() {
    let original = Artifact::File {
        artifact_uid: "artifact-file-1".to_string(),
        filepath: "outputs/report.txt".to_string(),
        filename: "report.txt".to_string(),
        mime_type: "text/plain".to_string(),
        description: Some("Daily summary".to_string()),
        size_bytes: Some(42),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_vec_serialize_deserialize_roundtrip() {
    let original = vec![
        Artifact::Plan {
            document_uid: "doc-1".to_string(),
            notebook_uid: None,
            title: Some("Plan 1".to_string()),
        },
        Artifact::PullRequest {
            url: "https://github.com/org/repo/pull/1".to_string(),
            branch: "main".to_string(),
            repo: Some("repo".to_string()),
            number: Some(1),
        },
        Artifact::File {
            artifact_uid: "artifact-file-1".to_string(),
            filepath: "outputs/report.txt".to_string(),
            filename: "report.txt".to_string(),
            mime_type: "text/plain".to_string(),
            description: Some("Daily summary".to_string()),
            size_bytes: Some(42),
        },
    ];

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Vec<Artifact> = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}
