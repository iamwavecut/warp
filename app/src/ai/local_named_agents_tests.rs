#![cfg(not(target_family = "wasm"))]

use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;
use warp_cli::agent::Harness;

use super::{
    AgentBundleSecretRefs, LocalNamedAgentRepository, NamedAgentBundle, NamedAgentError,
    NamedAgentList, NamedAgentRunOverrides, merge_named_agent_config,
};
use crate::ai::agent_sdk::config_file::AgentConfigSnapshotFile;
use crate::ai::ambient_agents::AgentConfigSnapshot;

fn bundle(name: &str) -> NamedAgentBundle {
    NamedAgentBundle {
        name: name.to_owned(),
        description: Some("local test agent".to_owned()),
        base_prompt: Some("bundle prompt".to_owned()),
        model_id: "custom/local/code".to_owned(),
        profile_id: Some("profile-1".to_owned()),
        skills: vec!["review".to_owned(), "format".to_owned()],
        mcp_servers: Some(BTreeMap::from([(
            "filesystem".to_owned(),
            json!({"command": "local-mcp", "args": []}),
        )])),
        harness: Harness::Oz,
        computer_use_enabled: Some(false),
        secret_refs: None,
    }
}

#[test]
fn local_named_agent_repository_keeps_uuid_when_name_changes_and_restarts() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let created = repository.create(bundle("Review")).unwrap();
    let id = created.id();
    assert_eq!(created.path(), dir.path().join(format!("{id}.yaml")));

    let mut renamed = created.bundle().clone();
    renamed.name = "Renamed".to_owned();
    let updated = repository.update(id, created.revision(), renamed).unwrap();
    assert_eq!(updated.id(), id);
    assert_eq!(updated.bundle().name, "Renamed");

    let restarted = LocalNamedAgentRepository::new(dir.path());
    let loaded = restarted.resolve(&id.to_string()).unwrap();
    assert_eq!(loaded.id(), id);
    assert_eq!(loaded.bundle().name, "Renamed");
}

#[test]
fn local_named_agent_listing_keeps_valid_files_when_another_file_is_bad() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let valid = repository.create(bundle("Valid")).unwrap();
    let bad_id = Uuid::new_v4();
    fs::write(
        dir.path().join(format!("{bad_id}.yaml")),
        "name: Broken\nmodel_id: custom/local/code\nunknown: true\n",
    )
    .unwrap();

    let NamedAgentList { agents, errors } = repository.list_with_errors().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id(), valid.id());
    assert_eq!(errors.len(), 1);
    assert!(errors[0].to_string().contains("unknown"));
    assert!(!errors[0].to_string().contains("bundle prompt"));
}

#[test]
fn local_named_agent_rejects_literal_secrets_and_mcp_env_values() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());

    let mut literal = bundle("Literal");
    literal.secret_refs = Some(AgentBundleSecretRefs {
        env_vars: BTreeMap::from([("OPENAI_API_KEY".to_owned(), "sk-inline".to_owned())]),
        keychain_entries: vec![],
    });
    let error = repository.create(literal).unwrap_err();
    assert!(matches!(error, NamedAgentError::SecretValueRejected { .. }));
    assert!(!error.to_string().contains("sk-inline"));

    let mut mcp = bundle("MCP secret");
    mcp.mcp_servers = Some(BTreeMap::from([(
        "server".to_owned(),
        json!({"command": "local-mcp", "env": {"API_TOKEN": "literal"}}),
    )]));
    let error = repository.create(mcp).unwrap_err();
    assert!(matches!(error, NamedAgentError::SecretValueRejected { .. }));
    assert!(!error.to_string().contains("literal"));

    let mut inline_url = bundle("Inline URL");
    inline_url.mcp_servers = Some(BTreeMap::from([(
        "server".to_owned(),
        json!({"url": "https://example.test/sse?api_key=abcdefghijklmnop"}),
    )]));
    let error = repository.create(inline_url).unwrap_err();
    assert!(matches!(error, NamedAgentError::SecretValueRejected { .. }));

    let mut env_header = bundle("Environment header");
    env_header.mcp_servers = Some(BTreeMap::from([(
        "server".to_owned(),
        json!({
            "url": "https://example.test/sse",
            "headers": {"Authorization": "Bearer ${MCP_TOKEN}"}
        }),
    )]));
    repository.create(env_header).unwrap();
}

#[test]
fn local_named_agent_update_is_compare_and_swap_and_delete_rejects_traversal() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let created = repository.create(bundle("CAS")).unwrap();

    let stale = created.revision().to_owned();
    let mut first = created.bundle().clone();
    first.description = Some("first".to_owned());
    let current = repository
        .update(created.id(), created.revision(), first)
        .unwrap();

    let mut second = current.bundle().clone();
    second.description = Some("second".to_owned());
    let error = repository.update(created.id(), &stale, second).unwrap_err();
    assert!(matches!(error, NamedAgentError::Conflict { .. }));
    assert_eq!(
        repository
            .get(created.id())
            .unwrap()
            .bundle()
            .description
            .as_deref(),
        Some("first")
    );

    let error = repository.delete("../outside", None).unwrap_err();
    assert!(matches!(error, NamedAgentError::InvalidSelector { .. }));
    repository
        .delete(&created.id().to_string(), Some(current.revision()))
        .unwrap();
    assert!(repository.get(created.id()).is_err());
}

#[test]
fn local_named_agent_merge_has_deterministic_precedence_without_mutating_bundle() {
    let source = bundle("Source");
    let one_shot = AgentConfigSnapshotFile {
        model_id: Some("custom/one-shot/model".to_owned()),
        base_prompt: Some("one-shot prompt".to_owned()),
        ..Default::default()
    };
    let cli = AgentConfigSnapshot {
        model_id: Some("custom/cli/model".to_owned()),
        computer_use_enabled: Some(true),
        ..Default::default()
    };
    let overrides = NamedAgentRunOverrides {
        one_shot: Some(one_shot),
        cli,
        bundle_skill_instructions: None,
        invoked_skill_instructions: Some("invoked skill".to_owned()),
    };

    let merged = merge_named_agent_config(&source, &overrides).unwrap();
    assert_eq!(merged.model_id.as_deref(), Some("custom/cli/model"));
    assert_eq!(merged.base_prompt.as_deref(), Some("invoked skill"));
    assert_eq!(merged.computer_use_enabled, Some(true));
    assert_eq!(source.model_id, "custom/local/code");
    assert_eq!(source.base_prompt.as_deref(), Some("bundle prompt"));
}

#[test]
fn local_named_agent_list_output_redacts_prompt_and_secret_references() {
    let mut agent = bundle("Private");
    agent.secret_refs = Some(AgentBundleSecretRefs {
        env_vars: BTreeMap::from([("API_KEY".to_owned(), "ENV_NAME".to_owned())]),
        keychain_entries: vec!["provider-key".to_owned()],
    });
    let output = super::format_named_agent_list(&[super::NamedAgentRecord::from_parts(
        Uuid::new_v4(),
        agent,
    )]);
    assert!(output.contains("Private"));
    assert!(!output.contains("bundle prompt"));
    assert!(!output.contains("ENV_NAME"));
    assert!(!output.contains("provider-key"));
    assert!(output.contains("local named agent"));
}
