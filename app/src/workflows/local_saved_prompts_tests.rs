use std::fs;

use tempfile::TempDir;
use uuid::Uuid;

use super::*;
use crate::workflows::workflow::Workflow;

fn repository() -> (TempDir, LocalSavedPromptRepository) {
    let dir = tempfile::tempdir().expect("temporary workflows directory");
    let repository = LocalSavedPromptRepository::new(dir.path());
    (dir, repository)
}

fn prompt(name: &str, query: &str) -> Workflow {
    Workflow::AgentMode {
        name: name.to_owned(),
        query: query.to_owned(),
        description: Some("description".to_owned()),
        arguments: vec![],
    }
}

#[test]
fn create_update_rename_delete_round_trip_keeps_uuid() {
    let (_dir, repository) = repository();
    let created = repository.create(prompt("first", "hello")).unwrap();
    assert!(created.path().starts_with(repository.managed_dir()));
    assert_eq!(
        created.path().extension().and_then(|ext| ext.to_str()),
        Some("yaml")
    );

    let reloaded = repository.get(created.id()).unwrap().unwrap();
    assert_eq!(reloaded.workflow(), &prompt("first", "hello"));

    let updated = repository
        .update(created.id(), prompt("renamed", "goodbye"))
        .unwrap();
    assert_eq!(updated.id(), created.id());
    assert_eq!(
        repository.get(created.id()).unwrap().unwrap().workflow(),
        &prompt("renamed", "goodbye")
    );

    repository.delete(created.id()).unwrap();
    assert!(repository.get(created.id()).unwrap().is_none());
}

#[test]
fn restart_reload_resolves_uuid_and_unique_exact_name() {
    let (dir, repository) = repository();
    let created = repository.create(prompt("Prompt", "hello")).unwrap();
    let restarted = LocalSavedPromptRepository::new(dir.path());

    assert_eq!(
        restarted.resolve(&created.id().to_string()).unwrap().id(),
        created.id()
    );
    assert_eq!(restarted.resolve("Prompt").unwrap().id(), created.id());
}

#[test]
fn duplicate_exact_names_are_ambiguous_without_private_contents() {
    let (_dir, repository) = repository();
    repository.create(prompt("same", "one")).unwrap();
    repository.create(prompt("same", "two")).unwrap();

    let error = repository.resolve("same").unwrap_err();
    assert!(
        matches!(error, LocalSavedPromptRepositoryError::AmbiguousName { ref name } if name == "same")
    );
    assert!(!error.to_string().contains("one"));
    assert!(!error.to_string().contains("two"));
}

#[test]
fn identical_content_keeps_distinct_managed_ids() {
    let (_dir, repository) = repository();
    let workflow = prompt("same", "identical");
    let first = repository.create(workflow.clone()).unwrap();
    let second = repository.create(workflow).unwrap();

    assert_ne!(first.id(), second.id());
    assert_eq!(
        repository.get(first.id()).unwrap().unwrap().id(),
        first.id()
    );
    assert_eq!(
        repository.get(second.id()).unwrap().unwrap().id(),
        second.id()
    );
    assert_eq!(
        repository.resolve(&first.id().to_string()).unwrap().id(),
        first.id()
    );
    assert_eq!(
        repository.resolve(&second.id().to_string()).unwrap().id(),
        second.id()
    );
}

#[test]
fn atomic_temporary_paths_are_not_effective_yaml_files() {
    assert!(is_atomic_temp_path(std::path::Path::new(
        ".prompt.yaml.tmp-123"
    )));
    assert!(!is_atomic_temp_path(std::path::Path::new("prompt.yaml")));
}

#[test]
fn agent_mode_metadata_round_trips_without_reinference() {
    let (_dir, repository) = repository();
    let workflow = Workflow::AgentMode {
        name: "metadata".to_owned(),
        query: "review {{target}}".to_owned(),
        description: Some("Keep this description".to_owned()),
        arguments: vec![
            crate::workflows::workflow::Argument::new("target", Default::default())
                .with_description("Description survives")
                .with_default("src/"),
        ],
    };
    let created = repository.create(workflow.clone()).unwrap();
    assert_eq!(
        repository.get(created.id()).unwrap().unwrap().workflow(),
        &workflow
    );
}

#[test]
fn command_workflows_and_multidocument_files_are_read_only() {
    let (_dir, repository) = repository();
    let command_path = repository
        .managed_dir()
        .join(format!("{}.yaml", Uuid::new_v4()));
    fs::create_dir_all(repository.managed_dir()).unwrap();
    fs::write(&command_path, "name: command\ncommand: echo hi\n").unwrap();
    let multi_path = repository
        .managed_dir()
        .join(format!("{}.yaml", Uuid::new_v4()));
    fs::write(
        &multi_path,
        "---\ntype: agent_mode\nname: one\nquery: one\n---\ntype: agent_mode\nname: two\nquery: two\n",
    )
    .unwrap();

    assert!(
        repository
            .delete(Uuid::parse_str(command_path.file_stem().unwrap().to_str().unwrap()).unwrap())
            .is_err()
    );
    assert!(
        repository
            .update(
                Uuid::parse_str(multi_path.file_stem().unwrap().to_str().unwrap()).unwrap(),
                prompt("replacement", "query")
            )
            .is_err()
    );
    assert!(command_path.exists());
    assert!(multi_path.exists());
}

#[test]
fn selector_cannot_escape_managed_directory() {
    let (_dir, repository) = repository();
    let outside = repository
        .managed_dir()
        .parent()
        .unwrap()
        .join("outside.yaml");
    fs::write(&outside, "type: agent_mode\nname: outside\nquery: secret\n").unwrap();

    assert!(repository.resolve("../outside").is_err());
    assert!(repository.resolve(outside.to_str().unwrap()).is_err());
    assert!(outside.exists());
}
