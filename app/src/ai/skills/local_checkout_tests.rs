use super::*;

fn git(cwd: &Path, args: &[&str]) {
    let output = command::blocking::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn cached_checkouts_refresh_full_history_and_remain_independent() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-b", "main"]);
    git(&source, &["config", "user.name", "Test"]);
    git(&source, &["config", "user.email", "test@example.invalid"]);
    std::fs::write(source.join("file"), "first").unwrap();
    git(&source, &["add", "file"]);
    git(&source, &["commit", "-m", "first"]);
    git(&source, &["branch", "other"]);
    let url = source.to_str().unwrap();
    let mirrors = temp.path().join("mirrors");
    let first = temp.path().join("first");
    clone_with_mirror(url, &first, &mirrors).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(first.join("file")).unwrap(),
        "first"
    );
    git(&first, &["rev-parse", "origin/other"]);
    std::fs::write(source.join("file"), "second").unwrap();
    git(&source, &["commit", "-am", "second"]);
    let second = temp.path().join("second");
    clone_with_mirror(url, &second, &mirrors).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(second.join("file")).unwrap(),
        "second"
    );
    git(&second, &["rev-parse", "HEAD~1"]);
    assert!(!second.join(".git/objects/info/alternates").exists());
    let origin = command::blocking::Command::new("git")
        .current_dir(&second)
        .args(["remote", "get-url", "origin"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(origin.stdout).unwrap().trim(), url);
    assert!(clone_with_mirror(url, &second, &mirrors).await.is_err());
    assert_eq!(
        std::fs::read_to_string(first.join("file")).unwrap(),
        "first"
    );
}

#[test]
fn repository_names_cannot_escape_the_local_checkout_directory() {
    for value in ["", "..", ".", "../repo", "/repo", "repo\\child", "--option"] {
        assert!(!valid_repository_component(value), "{value}");
    }
    assert!(valid_repository_component("org-name"));
    assert!(valid_repository_component("my.repo_1"));
}
