use std::fs;
use std::path::Path;

use tempfile::TempDir;
use uuid::Uuid;
use warp_cli::agent::Harness;

use super::*;

fn record(
    run_id: Uuid,
    session_id: Uuid,
    locator: Option<TranscriptLocator>,
) -> LocalHarnessRecord {
    LocalHarnessRecord::new(
        run_id,
        Harness::Codex,
        session_id,
        Path::new("/tmp/project"),
        locator,
        None,
    )
}

#[test]
fn local_harness_resume_repository_round_trips_metadata_without_payload() {
    let tmp = TempDir::new().unwrap();
    let repository = LocalHarnessRepository::new(tmp.path());
    let run_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let stored = repository.create(record(run_id, session_id, None)).unwrap();

    let path = repository.path_for_id(run_id);
    let json = fs::read_to_string(path).unwrap();
    assert!(!json.contains("prompt"));
    assert!(!json.contains("secret"));
    assert_eq!(repository.read(run_id).unwrap(), stored);
    assert_eq!(stored.schema_version, LOCAL_HARNESS_SCHEMA_VERSION);
    assert_eq!(stored.run_id, run_id);
    assert_eq!(stored.harness_session_id, session_id);
}

#[test]
fn local_harness_resume_repository_rejects_stale_compare_and_swap() {
    let tmp = TempDir::new().unwrap();
    let repository = LocalHarnessRepository::new(tmp.path());
    let run_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let stored = repository.create(record(run_id, session_id, None)).unwrap();
    let mut next = stored.clone();
    next.terminal = true;
    let updated = repository.update(next, stored.revision).unwrap();

    let mut stale = stored;
    stale.complete = true;
    let error = repository.update(stale, 0).unwrap_err();
    assert!(matches!(error, LocalHarnessResumeError::Conflict { .. }));
    assert_eq!(repository.read(run_id).unwrap(), updated);
}

#[test]
fn local_harness_resume_rejects_path_traversal_before_reading_transcript() {
    let tmp = TempDir::new().unwrap();
    let repository = LocalHarnessRepository::new(tmp.path());
    let session_id = Uuid::new_v4();
    let locator = TranscriptLocator {
        root: TranscriptRoot::CodexSessions,
        relative_path: "../outside.jsonl".to_owned(),
    };
    let error = repository
        .validate_transcript(&record(Uuid::new_v4(), session_id, Some(locator)))
        .unwrap_err();
    assert!(matches!(
        error,
        LocalHarnessResumeError::UnsafeTranscriptPath { .. }
    ));
}

#[cfg(unix)]
#[test]
fn local_harness_resume_rejects_outside_root_symlink() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("sessions");
    fs::create_dir_all(&root).unwrap();
    let outside = tmp.path().join("outside.jsonl");
    let session_id = Uuid::new_v4();
    fs::write(
        &outside,
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\"}}}}\n"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, root.join("rollout.jsonl")).unwrap();
    let repository = LocalHarnessRepository::with_transcript_roots(
        tmp.path().join("index"),
        root.clone(),
        tmp.path().join("claude-projects"),
    );
    let record = LocalHarnessRecord::new(
        Uuid::new_v4(),
        Harness::Codex,
        session_id,
        Path::new("/tmp/project"),
        Some(TranscriptLocator {
            root: TranscriptRoot::CodexSessions,
            relative_path: "rollout.jsonl".to_owned(),
        }),
        None,
    );
    let error = repository.validate_transcript(&record).unwrap_err();
    assert!(matches!(
        error,
        LocalHarnessResumeError::UnsafeTranscriptPath { .. }
    ));
}

#[test]
fn local_harness_resume_validates_codex_session_uuid() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("sessions");
    let session_id = Uuid::new_v4();
    let day = root.join("2026/08/24");
    fs::create_dir_all(&day).unwrap();
    let path = day.join(format!("rollout-test-{session_id}.jsonl"));
    fs::write(
        &path,
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\"}}}}\n"),
    )
    .unwrap();
    let repository = LocalHarnessRepository::with_transcript_roots(
        tmp.path().join("index"),
        root.clone(),
        tmp.path().join("claude-projects"),
    );
    let relative_path = path
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let record = LocalHarnessRecord::new(
        Uuid::new_v4(),
        Harness::Codex,
        session_id,
        Path::new("/tmp/project"),
        Some(TranscriptLocator {
            root: TranscriptRoot::CodexSessions,
            relative_path,
        }),
        None,
    );
    assert_eq!(
        repository.validate_transcript(&record).unwrap(),
        fs::canonicalize(path).unwrap()
    );
}

#[test]
fn local_harness_resume_validates_claude_session_uuid() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("claude-projects");
    let session_id = Uuid::new_v4();
    let path = projects.join(format!("project/{session_id}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!("{{\"sessionId\":\"{session_id}\",\"type\":\"user\"}}\n"),
    )
    .unwrap();
    let repository = LocalHarnessRepository::with_transcript_roots(
        tmp.path().join("index"),
        tmp.path().join("codex-sessions"),
        projects.clone(),
    );
    let relative_path = path
        .strip_prefix(&projects)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let mut record = record(Uuid::new_v4(), session_id, None);
    record.harness = Harness::Claude;
    record.transcript = Some(TranscriptLocator {
        root: TranscriptRoot::ClaudeProjects,
        relative_path,
    });
    assert_eq!(
        repository.validate_transcript(&record).unwrap(),
        fs::canonicalize(path).unwrap()
    );
}

#[test]
fn local_harness_resume_missing_transcript_is_nonfatal_for_periodic_save() {
    let tmp = TempDir::new().unwrap();
    let repository = LocalHarnessRepository::new(tmp.path());
    let record = record(Uuid::new_v4(), Uuid::new_v4(), None);
    assert!(repository.discover_transcript(&record).unwrap().is_none());
}
