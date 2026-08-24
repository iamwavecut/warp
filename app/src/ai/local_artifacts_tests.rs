use std::fs;

use super::*;

fn repository() -> (tempfile::TempDir, LocalArtifactRepository) {
    let directory = tempfile::tempdir().expect("temp directory");
    let repository = LocalArtifactRepository::in_directory(directory.path()).expect("repository");
    (directory, repository)
}

#[test]
fn p2_5_local_artifact_metadata_and_bytes_survive_restart() {
    let (directory, repository) = repository();
    let source = directory.path().join("screenshot.png");
    fs::write(
        &source,
        b"\x89PNG\r\n\x1a\nlocal screenshot artifact contents",
    )
    .expect("source");
    let owner = LocalArtifactOwner::conversation("conversation-a");
    let imported = repository
        .import_path(&source, owner, Some("Local preview".to_string()))
        .expect("import");

    assert_eq!(imported.kind, LocalArtifactKind::Screenshot);
    assert_eq!(imported.mime_type, "image/png");
    assert_eq!(imported.checksum_sha256.len(), 64);
    assert_ne!(imported.local_path, source);
    assert!(imported.local_path.starts_with(repository.root()));

    drop(repository);
    let reopened = LocalArtifactRepository::in_directory(directory.path()).expect("reopen");
    let restored = reopened
        .resolve_verified_path(imported.artifact_uid)
        .expect("verified artifact");
    assert_eq!(restored, imported);
    assert_eq!(
        fs::read(restored.local_path).expect("bytes"),
        fs::read(source).expect("source bytes")
    );
}

#[test]
fn p2_5_owner_release_keeps_shared_artifact_then_removes_last_copy() {
    let (directory, repository) = repository();
    let source = directory.path().join("report.txt");
    fs::write(&source, b"local report").expect("source");
    let owner_a = LocalArtifactOwner::conversation("conversation-a");
    let owner_b = LocalArtifactOwner::conversation("conversation-b");
    let imported = repository
        .import_path(&source, owner_a.clone(), None)
        .expect("import");
    assert!(
        repository
            .attach_owner_if_present(imported.artifact_uid, &owner_b)
            .expect("attach")
    );

    assert!(
        repository
            .release_owner(&owner_a)
            .expect("release a")
            .is_empty()
    );
    assert!(imported.local_path.exists());
    assert!(
        repository
            .get(imported.artifact_uid)
            .expect("get")
            .is_some()
    );

    assert_eq!(
        repository.release_owner(&owner_b).expect("release b"),
        vec![imported.artifact_uid]
    );
    assert!(!imported.local_path.exists());
    assert!(
        repository
            .get(imported.artifact_uid)
            .expect("get")
            .is_none()
    );
}

#[test]
fn p2_5_checksum_verification_detects_tampering() {
    let (directory, repository) = repository();
    let source = directory.path().join("report.txt");
    fs::write(&source, b"trusted local report").expect("source");
    let imported = repository
        .import_path(
            &source,
            LocalArtifactOwner::manual("test"),
            Some("Report".to_string()),
        )
        .expect("import");
    fs::write(&imported.local_path, b"tampered").expect("tamper");

    assert!(matches!(
        repository.resolve_verified_path(imported.artifact_uid),
        Err(LocalArtifactError::ChecksumMismatch { .. })
    ));
}
