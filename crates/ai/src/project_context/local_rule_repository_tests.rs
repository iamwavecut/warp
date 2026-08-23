use std::fs;

use tempfile::tempdir;

use super::*;

#[test]
fn creates_reads_updates_and_deletes_a_project_rule_with_revision_cas() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let target = fs::canonicalize(&root).unwrap().join("WARP.md");
    let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);

    let created = repository
        .create_project(&root, ProjectRuleFile::Warp, "first")
        .unwrap();
    assert_eq!(created.path, target);
    assert_eq!(created.content, "first");

    let updated = repository
        .update(&created.path, &created.revision, "second")
        .unwrap();
    assert_eq!(updated.content, "second");
    assert_ne!(updated.revision, created.revision);

    let read = repository.read(&target).unwrap();
    assert_eq!(read.content, "second");

    repository.delete(&target, &read.revision).unwrap();
    assert!(!target.exists());
}

#[test]
fn creates_reads_updates_and_deletes_the_managed_global_rule() {
    let temp = tempdir().unwrap();
    let target = temp.path().join(".agents/AGENTS.md");
    let mut repository = LocalRuleRepository::new_for_test([target.clone()], Vec::new());

    let created = repository.create_global("global").unwrap();
    assert_eq!(
        created.path,
        fs::canonicalize(target.parent().unwrap())
            .unwrap()
            .join("AGENTS.md")
    );
    assert_eq!(created.content, "global");

    let updated = repository
        .update(&created.path, &created.revision, "updated")
        .unwrap();
    repository.delete(&updated.path, &updated.revision).unwrap();
    assert!(!updated.path.exists());
}

#[test]
fn rejects_concurrent_external_edit_without_overwriting_it() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let target = root.join("WARP.md");
    fs::write(&target, "original").unwrap();
    let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
    let opened = repository.read(&target).unwrap();
    fs::write(&target, "external").unwrap();

    let error = repository
        .update(&target, &opened.revision, "draft")
        .unwrap_err();
    assert!(matches!(error, LocalRuleError::Conflict { .. }));
    assert_eq!(fs::read_to_string(target).unwrap(), "external");
}

#[test]
fn rejects_paths_that_are_not_surfaced_or_escape_the_project_root() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let target = root.join("WARP.md");
    fs::write(&target, "content").unwrap();
    let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);

    assert!(matches!(
        repository.read(&temp.path().join("other.md")),
        Err(LocalRuleError::NotSurfaced { .. })
    ));
    assert!(matches!(
        repository.create_project(&root.join(".."), ProjectRuleFile::Warp, "bad"),
        Err(LocalRuleError::InvalidPath { .. })
    ));
}

#[cfg(unix)]
#[test]
fn rejects_symlink_replacement_before_edit() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let outside = temp.path().join("outside.md");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("WARP.md"), "inside").unwrap();
    fs::write(&outside, "outside").unwrap();
    let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
    let target = root.join("WARP.md");
    let opened = repository.read(&target).unwrap();
    fs::remove_file(&target).unwrap();
    symlink(&outside, &target).unwrap();

    let error = repository
        .update(&target, &opened.revision, "draft")
        .unwrap_err();
    assert!(matches!(error, LocalRuleError::SymlinkEscape { .. }));
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
}
