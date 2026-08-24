use std::fs;
use std::path::Path;

use serde_json::Value;
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

#[cfg(unix)]
#[test]
fn local_harness_resume_rejects_swapped_transcript_parent() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("sessions");
    let parent = root.join("project");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&parent).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let session_id = Uuid::new_v4();
    let transcript = parent.join(format!("{session_id}.jsonl"));
    fs::write(
        &transcript,
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\"}}}}\n"),
    )
    .unwrap();
    fs::rename(&parent, tmp.path().join("moved")).unwrap();
    std::os::unix::fs::symlink(&outside, &parent).unwrap();
    let repository = LocalHarnessRepository::with_transcript_roots(
        tmp.path().join("index"),
        root,
        tmp.path().join("claude-projects"),
    );
    let record = LocalHarnessRecord::new(
        Uuid::new_v4(),
        Harness::Codex,
        session_id,
        Path::new("/tmp/project"),
        Some(TranscriptLocator {
            root: TranscriptRoot::CodexSessions,
            relative_path: format!("project/{session_id}.jsonl"),
        }),
        None,
    );
    assert!(matches!(
        repository.validate_transcript(&record),
        Err(LocalHarnessResumeError::UnsafeTranscriptPath { .. }
            | LocalHarnessResumeError::MalformedTranscript { .. })
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
    let working_dir = tmp.path().join("workspace");
    fs::create_dir_all(&working_dir).unwrap();
    let working_dir = fs::canonicalize(working_dir).unwrap();
    let projects = tmp.path().join("claude-projects");
    let session_id = Uuid::new_v4();
    let path = projects
        .join(encode_claude_cwd(&working_dir))
        .join(format!("{session_id}.jsonl"));
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
    let mut record = LocalHarnessRecord::new(
        Uuid::new_v4(),
        Harness::Claude,
        session_id,
        &working_dir,
        None,
        None,
    );
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

#[test]
fn local_harness_resume_rejects_claude_message_uuid_without_session_id() {
    let tmp = TempDir::new().unwrap();
    let working_dir = tmp.path().join("workspace");
    fs::create_dir_all(&working_dir).unwrap();
    let working_dir = fs::canonicalize(working_dir).unwrap();
    let projects = tmp.path().join("claude-projects");
    let session_id = Uuid::new_v4();
    let message_id = Uuid::new_v4();
    let project_name = encode_claude_cwd(&working_dir);
    let path = projects
        .join(&project_name)
        .join(format!("{session_id}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(r#"{{"type":"user","uuid":"{message_id}"}}"#) + "\n",
    )
    .unwrap();
    let repository = LocalHarnessRepository::with_transcript_roots(
        tmp.path().join("index"),
        tmp.path().join("codex-sessions"),
        projects.clone(),
    );
    let mut record = LocalHarnessRecord::new(
        Uuid::new_v4(),
        Harness::Claude,
        session_id,
        &working_dir,
        None,
        None,
    );
    record.transcript = Some(TranscriptLocator {
        root: TranscriptRoot::ClaudeProjects,
        relative_path: format!("{project_name}/{session_id}.jsonl"),
    });
    assert!(matches!(
        repository.validate_transcript(&record),
        Err(LocalHarnessResumeError::TranscriptSessionMismatch { .. })
    ));
}

#[test]
fn local_harness_resume_rejects_claude_locator_for_a_different_working_directory() {
    let tmp = TempDir::new().unwrap();
    let first_working_dir = tmp.path().join("first");
    let second_working_dir = tmp.path().join("second");
    fs::create_dir_all(&first_working_dir).unwrap();
    fs::create_dir_all(&second_working_dir).unwrap();
    let first_working_dir = fs::canonicalize(first_working_dir).unwrap();
    let second_working_dir = fs::canonicalize(second_working_dir).unwrap();
    let projects = tmp.path().join("claude-projects");
    let session_id = Uuid::new_v4();
    let project_name = encode_claude_cwd(&first_working_dir);
    let path = projects
        .join(&project_name)
        .join(format!("{session_id}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!("{{\"sessionId\":\"{session_id}\",\"type\":\"user\"}}\n"),
    )
    .unwrap();
    let repository = LocalHarnessRepository::with_transcript_roots(
        tmp.path().join("index"),
        tmp.path().join("codex-sessions"),
        projects,
    );
    let record = LocalHarnessRecord::new(
        Uuid::new_v4(),
        Harness::Claude,
        session_id,
        &second_working_dir,
        Some(TranscriptLocator {
            root: TranscriptRoot::ClaudeProjects,
            relative_path: format!("{project_name}/{session_id}.jsonl"),
        }),
        None,
    );
    assert!(matches!(
        repository.validate_transcript(&record),
        Err(LocalHarnessResumeError::ClaudeSessionIndexConflict { .. })
    ));
}

#[test]
fn local_harness_resume_advisory_lock_recovers_from_stale_lock_path() {
    let tmp = TempDir::new().unwrap();
    let repository = LocalHarnessRepository::new(tmp.path());
    let run_id = Uuid::new_v4();
    fs::create_dir_all(tmp.path()).unwrap();
    fs::write(tmp.path().join(format!(".{run_id}.lock")), b"").unwrap();

    repository
        .create(record(run_id, Uuid::new_v4(), None))
        .unwrap();
}

#[test]
fn local_harness_resume_validates_existing_canonical_working_directory() {
    let tmp = TempDir::new().unwrap();
    let repository = LocalHarnessRepository::new(tmp.path().join("index"));
    let working_dir = tmp.path().join("project");
    fs::create_dir_all(&working_dir).unwrap();
    assert_eq!(
        repository.canonical_working_dir(&working_dir).unwrap(),
        fs::canonicalize(working_dir).unwrap()
    );
}

#[test]
fn local_harness_resume_upserts_canonical_claude_sessions_index() {
    let tmp = TempDir::new().unwrap();
    let working_dir = tmp.path().join("workspace");
    fs::create_dir_all(&working_dir).unwrap();
    let projects = tmp.path().join("claude-projects");
    let session_id = Uuid::new_v4();
    let canonical_working_dir = fs::canonicalize(&working_dir).unwrap();
    let project_dir = projects.join(encode_claude_cwd(&canonical_working_dir));
    fs::create_dir_all(&project_dir).unwrap();
    let transcript = project_dir.join(format!("{session_id}.jsonl"));
    fs::write(
        &transcript,
        format!(r#"{{"type":"user","sessionId":"{session_id}"}}"#) + "\n",
    )
    .unwrap();
    let repository = LocalHarnessRepository::with_transcript_roots(
        tmp.path().join("index"),
        tmp.path().join("codex-sessions"),
        projects.clone(),
    );
    let record = LocalHarnessRecord::new(
        Uuid::new_v4(),
        Harness::Claude,
        session_id,
        &working_dir,
        None,
        None,
    );

    repository
        .upsert_claude_sessions_index(&record, &transcript)
        .unwrap();
    let index: Value =
        serde_json::from_slice(&fs::read(project_dir.join("sessions-index.json")).unwrap())
            .unwrap();
    assert_eq!(index["version"], 1);
    assert_eq!(index["entries"][0]["sessionId"], session_id.to_string());
    assert_eq!(
        index["entries"][0]["projectPath"],
        fs::canonicalize(working_dir)
            .unwrap()
            .to_string_lossy()
            .to_string()
    );
    assert_eq!(index["entries"][0]["isSidechain"], false);
}

#[test]
fn local_harness_resume_help_requires_harness_specific_syntax() {
    assert!(resume_cli_help_is_compatible(Harness::Codex, "Commands:\n  resume").is_ok());
    assert!(resume_cli_help_is_compatible(Harness::Claude, "--resume <session-id>").is_ok());
    assert!(resume_cli_help_is_compatible(Harness::Claude, "uuid <message-id>").is_err());
}
