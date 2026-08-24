use tempfile::tempdir;

use super::*;

#[test]
fn crud_is_compare_and_swap_versioned_and_survives_restart() {
    let temp = tempdir().unwrap();
    let database = temp.path().join("warp.sqlite");
    let repository = LocalMemoryRepository::open(&database).unwrap();
    let created = repository
        .create(LocalMemoryScope::Global, "Editor", "Use Helix")
        .unwrap();
    let updated = repository
        .update(
            created.id,
            created.revision,
            LocalMemoryScope::Global,
            "Editor",
            "Use Neovim",
        )
        .unwrap();
    assert!(matches!(
        repository.update(
            created.id,
            created.revision,
            LocalMemoryScope::Global,
            "Editor",
            "stale",
        ),
        Err(LocalMemoryError::Conflict { .. })
    ));
    drop(repository);

    let restarted = LocalMemoryRepository::open(&database).unwrap();
    assert_eq!(
        restarted.get(created.id).unwrap().unwrap().content,
        "Use Neovim"
    );
    restarted.delete(updated.id, updated.revision).unwrap();
    assert_eq!(restarted.get(created.id).unwrap(), None);
    let history = restarted.history(created.id).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].operation, LocalMemoryOperation::Created);
    assert_eq!(history[1].operation, LocalMemoryOperation::Updated);
    assert_eq!(history[2].operation, LocalMemoryOperation::Deleted);
    assert_eq!(history[2].revision, 3);
}

#[test]
fn keyword_retrieval_is_ranked_bounded_and_scope_aware() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let other = temp.path().join("other");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::create_dir_all(&other).unwrap();
    let repository = LocalMemoryRepository::in_memory().unwrap();
    repository
        .create(
            LocalMemoryScope::Global,
            "Rust formatter",
            "Use cargo fmt for Rust files",
        )
        .unwrap();
    repository
        .create(
            LocalMemoryScope::Project {
                root: project.clone(),
            },
            "Project formatter",
            "This repository uses dprint",
        )
        .unwrap();
    repository
        .create(
            LocalMemoryScope::Project { root: other },
            "Other formatter",
            "This repository uses prettier",
        )
        .unwrap();

    let results = repository
        .search(
            "formatter for this Rust project",
            Some(&project.join("src")),
        )
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Rust formatter");
    assert!(
        results
            .iter()
            .any(|memory| memory.title == "Project formatter")
    );
    assert!(
        !results
            .iter()
            .any(|memory| memory.title == "Other formatter")
    );
    assert!(
        repository
            .search("unrelated", Some(&project))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn validation_rejects_empty_or_oversized_memory() {
    let repository = LocalMemoryRepository::in_memory().unwrap();
    assert!(matches!(
        repository.create(LocalMemoryScope::Global, "", "content"),
        Err(LocalMemoryError::EmptyTitle)
    ));
    assert!(matches!(
        repository.create(LocalMemoryScope::Global, "title", ""),
        Err(LocalMemoryError::EmptyContent)
    ));
    assert!(matches!(
        repository.create(
            LocalMemoryScope::Global,
            "title",
            &"x".repeat(MAX_CONTENT_CHARS + 1)
        ),
        Err(LocalMemoryError::ContentTooLong)
    ));
}
