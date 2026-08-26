#![cfg(not(target_family = "wasm"))]

use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;
use warp_cli::agent::Harness;

use super::{
    AgentBundleSecretRefs, LocalNamedAgentRepository, LocalNamedAgentRunMetadata,
    LocalNamedAgentRunStatus, NamedAgentBundle, NamedAgentError, NamedAgentList,
    NamedAgentRunOverrides, merge_named_agent_config, profile_sync_id,
};
use crate::ai::agent_sdk::config_file::AgentConfigSnapshotFile;
use crate::ai::ambient_agents::AgentConfigSnapshot;
use crate::server::ids::SyncId;

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
            "headers": {"Authorization": "${MCP_TOKEN}"}
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
fn local_named_agent_external_writer_cannot_bypass_revision_checks() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let created = repository.create(bundle("External writer")).unwrap();
    let path = created.path().to_owned();
    let mut external = created.bundle().clone();
    external.description = Some("written outside the repository".to_owned());
    fs::write(&path, serde_yaml::to_string(&external).unwrap()).unwrap();

    let error = repository
        .update(created.id(), created.revision(), external.clone())
        .unwrap_err();
    assert!(matches!(error, NamedAgentError::Conflict { .. }));
    let error = repository
        .delete(&created.id().to_string(), Some(created.revision()))
        .unwrap_err();
    assert!(matches!(error, NamedAgentError::Conflict { .. }));
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

#[test]
fn local_named_agent_accepts_server_and_client_profile_sync_ids() {
    assert!(matches!(
        profile_sync_id("Client-11111111-1111-1111-1111-111111111111").unwrap(),
        SyncId::ClientId(_)
    ));
    assert!(matches!(
        profile_sync_id("abcdefghijklmnopqrstuv").unwrap(),
        SyncId::ServerId(_)
    ));
}

#[test]
fn local_named_agent_rejects_skill_traversal_and_external_filename_shapes() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());

    for skill in [
        "org/repo:skill/name",
        "repo:../outside",
        "repo:/absolute",
        "repo:\\outside",
    ] {
        let mut candidate = bundle(skill);
        candidate.skills = vec![skill.to_owned()];
        assert!(matches!(
            repository.create(candidate),
            Err(NamedAgentError::InvalidBundle { .. })
        ));
    }

    let id = Uuid::new_v4();
    fs::write(
        dir.path().join(format!("{}.yml", id)),
        serde_yaml::to_string(&bundle("wrong extension")).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.path()
            .join(format!("{}.yaml", id.to_string().to_uppercase())),
        serde_yaml::to_string(&bundle("wrong case")).unwrap(),
    )
    .unwrap();
    let list = repository.list_with_errors().unwrap();
    assert_eq!(list.agents.len(), 0);
    assert_eq!(list.errors.len(), 2);
}

#[test]
fn local_named_agent_run_metadata_round_trips_without_prompt_or_secret_values() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let mut source = bundle("Metadata");
    source.secret_refs = Some(AgentBundleSecretRefs {
        env_vars: BTreeMap::from([("provider".to_owned(), "LOCAL_PROVIDER_KEY".to_owned())]),
        keychain_entries: vec!["provider-keychain-entry".to_owned()],
    });
    let record = repository.create(source).unwrap();
    let run_id = Uuid::new_v4();
    let effective = record.bundle().to_snapshot();
    let metadata =
        LocalNamedAgentRunMetadata::from_record(run_id, &record, &effective, dir.path()).unwrap();
    let path = repository.write_run_metadata(&metadata).unwrap();
    assert!(path.ends_with(format!("{run_id}.yaml")));
    let encoded = fs::read_to_string(path).unwrap();
    assert!(!encoded.contains("bundle prompt"));
    assert!(encoded.contains("LOCAL_PROVIDER_KEY"));
    assert!(encoded.contains("provider-keychain-entry"));

    let restarted = LocalNamedAgentRepository::new(dir.path());
    let restored = restarted.read_run_metadata(run_id).unwrap();
    assert_eq!(restored, metadata);
    assert_eq!(restored.named_agent_id, record.id());
    assert_eq!(restored.bundle_revision, record.revision());
    assert_eq!(restored.ordered_skills, record.bundle().skills);
    assert!(restored.effective_config.base_prompt.is_none());
    assert_eq!(restored.status, LocalNamedAgentRunStatus::Running);
    assert_eq!(restored.owner_pid, Some(std::process::id()));
    assert_eq!(
        restored
            .environment_snapshot
            .as_ref()
            .unwrap()
            .working_directory,
        dunce::canonicalize(dir.path()).unwrap()
    );
    assert!(
        restored
            .environment_snapshot
            .as_ref()
            .unwrap()
            .repository_root
            .is_none()
    );
    assert!(restored.completed_at.is_none());
}

#[test]
fn local_named_agent_run_status_is_updated_atomically() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let record = repository.create(bundle("Status")).unwrap();
    let run_id = Uuid::new_v4();
    let metadata = LocalNamedAgentRunMetadata::from_record(
        run_id,
        &record,
        &record.bundle().to_snapshot(),
        dir.path(),
    )
    .unwrap();
    repository.write_run_metadata(&metadata).unwrap();

    let completed = repository
        .mark_run_status(run_id, LocalNamedAgentRunStatus::Cancelled)
        .unwrap();

    assert_eq!(completed.status, LocalNamedAgentRunStatus::Cancelled);
    assert!(completed.completed_at.is_some());
    assert_eq!(completed.owner_pid, None);
    assert_eq!(repository.read_run_metadata(run_id).unwrap(), completed);
}

#[test]
fn stale_running_metadata_is_repaired_without_touching_a_live_run() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let record = repository.create(bundle("Repair")).unwrap();

    let stale_id = Uuid::new_v4();
    let mut stale = LocalNamedAgentRunMetadata::from_record(
        stale_id,
        &record,
        &record.bundle().to_snapshot(),
        dir.path(),
    )
    .unwrap();
    stale.owner_pid = None;
    repository.write_run_metadata(&stale).unwrap();

    let live_id = Uuid::new_v4();
    let live = LocalNamedAgentRunMetadata::from_record(
        live_id,
        &record,
        &record.bundle().to_snapshot(),
        dir.path(),
    )
    .unwrap();
    repository.write_run_metadata(&live).unwrap();

    assert_eq!(repository.repair_stale_runs().unwrap(), 1);
    assert_eq!(
        repository.read_run_metadata(stale_id).unwrap().status,
        LocalNamedAgentRunStatus::Interrupted
    );
    assert_eq!(
        repository.read_run_metadata(live_id).unwrap().status,
        LocalNamedAgentRunStatus::Running
    );
}

#[test]
fn local_environment_snapshot_captures_git_state_without_remote_data() {
    let dir = TempDir::new().unwrap();
    let git_repository = git2::Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("tracked.txt"), "local state\n").unwrap();
    let mut index = git_repository.index().unwrap();
    index.add_path(std::path::Path::new("tracked.txt")).unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = git_repository.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("WarpOss", "local@example.invalid").unwrap();
    let commit_id = git_repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "local snapshot",
            &tree,
            &[],
        )
        .unwrap();
    let expected_head_reference = git_repository
        .head()
        .unwrap()
        .shorthand()
        .unwrap()
        .to_owned();
    let working_dir = dir.path().join("nested");
    fs::create_dir(&working_dir).unwrap();

    let agents_dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(agents_dir.path());
    let record = repository.create(bundle("Git snapshot")).unwrap();
    let metadata = LocalNamedAgentRunMetadata::from_record(
        Uuid::new_v4(),
        &record,
        &record.bundle().to_snapshot(),
        &working_dir,
    )
    .unwrap();
    let snapshot = metadata.environment_snapshot.unwrap();

    assert_eq!(
        snapshot.repository_root,
        Some(dunce::canonicalize(dir.path()).unwrap())
    );
    assert_eq!(
        snapshot.working_directory_relative_to_repository,
        Some(std::path::PathBuf::from("nested"))
    );
    assert_eq!(snapshot.head_commit, Some(commit_id.to_string()));
    assert_eq!(
        snapshot.head_reference.as_deref(),
        Some(expected_head_reference.as_str())
    );
}

#[test]
fn local_named_agent_cleans_temporary_files_after_cas_errors() {
    let dir = TempDir::new().unwrap();
    let repository = LocalNamedAgentRepository::new(dir.path());
    let record = repository.create(bundle("Temp cleanup")).unwrap();
    let mut updated = record.bundle().clone();
    updated.description = Some("changed".to_owned());
    assert!(matches!(
        repository.update(record.id(), "stale", updated),
        Err(NamedAgentError::Conflict { .. })
    ));
    let leftovers = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-") || name.ends_with(".lock"))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "temporary files remain: {leftovers:?}"
    );
}
