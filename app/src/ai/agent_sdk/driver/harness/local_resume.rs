//! Local, transcript-only continuation state for third-party CLI harnesses.
//!
//! The third-party CLIs own their JSONL files. Warp persists only a small,
//! versioned locator under `data_dir()/harness-sessions`; no prompt, output,
//! credentials, or transcript contents are copied into that index.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use warp_cli::agent::Harness;

use crate::ai::ambient_agents::{AmbientAgentTaskId, task::HarnessModelConfig};

pub(crate) const LOCAL_HARNESS_SCHEMA_VERSION: u32 = 1;
const LOCAL_HARNESS_SESSIONS_DIR: &str = "harness-sessions";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_HOME_DIR: &str = ".codex";
const CODEX_SESSIONS_DIR: &str = "sessions";
const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
const CLAUDE_CONFIG_DIR: &str = ".claude";
const CLAUDE_PROJECTS_DIR: &str = "projects";
const JSON_EXTENSION: &str = "json";

/// Save points visible in the local index. A save point is advanced only after
/// its metadata and transcript validation has succeeded.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalHarnessSavePoint {
    SessionStart,
    Periodic,
    PostTurn,
    Final,
}

/// An allowlisted, root-relative transcript path. Absolute paths and `..`
/// components never enter the index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct TranscriptLocator {
    pub(crate) root: TranscriptRoot,
    pub(crate) relative_path: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TranscriptRoot {
    CodexSessions,
    ClaudeProjects,
}

/// The only durable data Warp writes for a third-party harness run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LocalHarnessRecord {
    pub(crate) schema_version: u32,
    /// Monotonically increasing per-run CAS revision.
    pub(crate) revision: u64,
    pub(crate) run_id: Uuid,
    pub(crate) harness: Harness,
    pub(crate) harness_session_id: Uuid,
    pub(crate) working_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transcript: Option<TranscriptLocator>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_save_point: Option<LocalHarnessSavePoint>,
    pub(crate) terminal: bool,
    pub(crate) complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) task_id: Option<AmbientAgentTaskId>,
}

impl LocalHarnessRecord {
    pub(crate) fn new(
        run_id: Uuid,
        harness: Harness,
        harness_session_id: Uuid,
        working_dir: &Path,
        transcript: Option<TranscriptLocator>,
        task_id: Option<AmbientAgentTaskId>,
    ) -> Self {
        let now = Utc::now();
        Self {
            schema_version: LOCAL_HARNESS_SCHEMA_VERSION,
            revision: 0,
            run_id,
            harness,
            harness_session_id,
            working_dir: working_dir.to_path_buf(),
            transcript,
            created_at: now,
            updated_at: now,
            last_save_point: None,
            terminal: false,
            complete: false,
            task_id,
        }
    }
}

/// Typed input handed to a harness runner. `session_id` is absent only for a
/// fresh Codex run, whose CLI emits its UUID after launch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalHarnessResumePayload {
    pub(crate) run_id: Uuid,
    pub(crate) harness: Harness,
    pub(crate) session_id: Option<Uuid>,
    pub(crate) working_dir: PathBuf,
    pub(crate) transcript: Option<TranscriptLocator>,
    pub(crate) task_id: Option<AmbientAgentTaskId>,
    pub(crate) is_resume: bool,
    pub(crate) model_config: Option<HarnessModelConfig>,
}

impl LocalHarnessResumePayload {
    pub(crate) fn fresh(
        run_id: Uuid,
        harness: Harness,
        working_dir: &Path,
        task_id: Option<AmbientAgentTaskId>,
        model_config: Option<HarnessModelConfig>,
    ) -> Self {
        Self {
            run_id,
            harness,
            session_id: None,
            working_dir: working_dir.to_path_buf(),
            transcript: None,
            task_id,
            is_resume: false,
            model_config,
        }
    }

    pub(crate) fn from_record(record: &LocalHarnessRecord) -> Self {
        Self {
            run_id: record.run_id,
            harness: record.harness,
            session_id: Some(record.harness_session_id),
            working_dir: record.working_dir.clone(),
            transcript: record.transcript.clone(),
            task_id: record.task_id,
            is_resume: true,
            model_config: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LocalHarnessResumeError {
    #[error("local harness resume record {run_id} does not exist")]
    MissingRecord { run_id: Uuid },
    #[error("local harness resume record {run_id} has unsupported schema version {version}")]
    UnsupportedSchema { run_id: Uuid, version: u32 },
    #[error("local harness resume record {run_id} is corrupt: {reason}")]
    CorruptRecord { run_id: Uuid, reason: String },
    #[error(
        "local harness resume record {run_id} changed concurrently (expected revision {expected}, actual {actual:?})"
    )]
    Conflict {
        run_id: Uuid,
        expected: u64,
        actual: Option<u64>,
    },
    #[error("local harness resume record {run_id} cannot use unsupported harness {harness}")]
    UnsupportedHarness { run_id: Uuid, harness: Harness },
    #[error("transcript locator is unsafe: {path}")]
    UnsafeTranscriptPath { path: String },
    #[error("transcript is missing: {path}")]
    MissingTranscript { path: PathBuf },
    #[error("transcript is malformed at {path}: {reason}")]
    MalformedTranscript { path: PathBuf, reason: String },
    #[error("transcript session UUID at {path} does not match indexed UUID {expected}")]
    TranscriptSessionMismatch { path: PathBuf, expected: Uuid },
    #[error("local harness resume I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not serialize local harness resume record: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Local repository with a per-run lock and atomic publication.
#[derive(Clone, Debug)]
pub(crate) struct LocalHarnessRepository {
    index_dir: PathBuf,
    codex_sessions_root: PathBuf,
    claude_projects_root: PathBuf,
}

impl LocalHarnessRepository {
    pub(crate) fn for_user() -> Self {
        let data_dir = warp_core::paths::data_dir();
        Self::with_transcript_roots(
            data_dir.join(LOCAL_HARNESS_SESSIONS_DIR),
            codex_sessions_root(),
            claude_projects_root(),
        )
    }

    pub(crate) fn new(index_dir: impl AsRef<Path>) -> Self {
        Self::with_transcript_roots(index_dir, codex_sessions_root(), claude_projects_root())
    }

    pub(crate) fn with_transcript_roots(
        index_dir: impl AsRef<Path>,
        codex_sessions_root: impl AsRef<Path>,
        claude_projects_root: impl AsRef<Path>,
    ) -> Self {
        Self {
            index_dir: index_dir.as_ref().to_path_buf(),
            codex_sessions_root: codex_sessions_root.as_ref().to_path_buf(),
            claude_projects_root: claude_projects_root.as_ref().to_path_buf(),
        }
    }

    pub(crate) fn path_for_id(&self, run_id: Uuid) -> PathBuf {
        self.index_dir.join(format!("{run_id}.{JSON_EXTENSION}"))
    }

    pub(crate) fn create(
        &self,
        mut record: LocalHarnessRecord,
    ) -> Result<LocalHarnessRecord, LocalHarnessResumeError> {
        self.ensure_index_dir()?;
        let lock = self.acquire_lock(record.run_id)?;
        let path = self.path_for_id(record.run_id);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(LocalHarnessResumeError::Conflict {
                    run_id: record.run_id,
                    expected: 0,
                    actual: self.read(record.run_id).ok().map(|value| value.revision),
                });
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(path, source)),
        }
        record.schema_version = LOCAL_HARNESS_SCHEMA_VERSION;
        record.revision = 1;
        self.write_atomically(&path, &record, false)?;
        drop(lock);
        self.read(record.run_id)
    }

    pub(crate) fn read(&self, run_id: Uuid) -> Result<LocalHarnessRecord, LocalHarnessResumeError> {
        let path = self.path_for_id(run_id);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(LocalHarnessResumeError::MissingRecord { run_id });
            }
            Err(source) => return Err(io_error(path, source)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LocalHarnessResumeError::CorruptRecord {
                run_id,
                reason: "record is not a regular file".to_owned(),
            });
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = options
            .open(&path)
            .map_err(|source| io_error(path.clone(), source))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| io_error(path.clone(), source))?;
        let record: LocalHarnessRecord = serde_json::from_slice(&bytes).map_err(|error| {
            LocalHarnessResumeError::CorruptRecord {
                run_id,
                reason: error.to_string(),
            }
        })?;
        if record.run_id != run_id {
            return Err(LocalHarnessResumeError::CorruptRecord {
                run_id,
                reason: format!("record contains run_id {}", record.run_id),
            });
        }
        if record.schema_version != LOCAL_HARNESS_SCHEMA_VERSION {
            return Err(LocalHarnessResumeError::UnsupportedSchema {
                run_id,
                version: record.schema_version,
            });
        }
        if record.revision == 0 {
            return Err(LocalHarnessResumeError::CorruptRecord {
                run_id,
                reason: "record revision is zero".to_owned(),
            });
        }
        Ok(record)
    }

    pub(crate) fn update(
        &self,
        mut record: LocalHarnessRecord,
        expected_revision: u64,
    ) -> Result<LocalHarnessRecord, LocalHarnessResumeError> {
        self.ensure_index_dir()?;
        let lock = self.acquire_lock(record.run_id)?;
        let current = self.read(record.run_id)?;
        if current.revision != expected_revision {
            drop(lock);
            return Err(LocalHarnessResumeError::Conflict {
                run_id: record.run_id,
                expected: expected_revision,
                actual: Some(current.revision),
            });
        }
        let latest = self.read(record.run_id)?;
        if latest.revision != expected_revision {
            drop(lock);
            return Err(LocalHarnessResumeError::Conflict {
                run_id: record.run_id,
                expected: expected_revision,
                actual: Some(latest.revision),
            });
        }
        record.schema_version = LOCAL_HARNESS_SCHEMA_VERSION;
        record.revision = expected_revision.saturating_add(1);
        record.created_at = current.created_at;
        record.updated_at = Utc::now();
        let path = self.path_for_id(record.run_id);
        self.write_atomically(&path, &record, true)?;
        drop(lock);
        self.read(record.run_id)
    }

    /// Explicit local-history deletion. This intentionally never touches the
    /// third-party transcript root.
    pub(crate) fn delete(&self, run_id: Uuid) -> Result<(), LocalHarnessResumeError> {
        self.ensure_index_dir()?;
        let lock = self.acquire_lock(run_id)?;
        let path = self.path_for_id(run_id);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                drop(lock);
                Err(LocalHarnessResumeError::CorruptRecord {
                    run_id,
                    reason: "record is not a regular file".to_owned(),
                })
            }
            Ok(_) => {
                fs::remove_file(&path).map_err(|source| io_error(path, source))?;
                drop(lock);
                Ok(())
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                drop(lock);
                Err(LocalHarnessResumeError::MissingRecord { run_id })
            }
            Err(source) => {
                drop(lock);
                Err(io_error(path, source))
            }
        }
    }

    /// Locate a CLI transcript for a save point. A missing transcript is
    /// normal during the first periodic save and is represented as `None`.
    pub(crate) fn discover_transcript(
        &self,
        record: &LocalHarnessRecord,
    ) -> Result<Option<(TranscriptLocator, PathBuf)>, LocalHarnessResumeError> {
        ensure_supported_harness(record)?;

        if let Some(locator) = &record.transcript {
            match self.validate_locator(record, locator) {
                Ok(path) => return Ok(Some((locator.clone(), path))),
                Err(LocalHarnessResumeError::MissingTranscript { .. }) => {}
                Err(error) => return Err(error),
            }
        }

        let found = match record.harness {
            Harness::Codex => {
                find_codex_transcript(&self.codex_sessions_root, record.harness_session_id)?
            }
            Harness::Claude => {
                let relative = PathBuf::from(encode_claude_cwd(&record.working_dir))
                    .join(format!("{}.jsonl", record.harness_session_id));
                let path = self.claude_projects_root.join(relative);
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                        return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
                            path: path.display().to_string(),
                        });
                    }
                    Ok(_) => Some(path),
                    Err(source) if source.kind() == io::ErrorKind::NotFound => None,
                    Err(source) => return Err(io_error(path, source)),
                }
            }
            unsupported => {
                return Err(LocalHarnessResumeError::UnsupportedHarness {
                    run_id: record.run_id,
                    harness: unsupported,
                });
            }
        };

        let Some(path) = found else { return Ok(None) };
        let root = match record.harness {
            Harness::Codex => &self.codex_sessions_root,
            Harness::Claude => &self.claude_projects_root,
            _ => unreachable!(),
        };
        let locator = locator_for_path(root, transcript_root(record.harness), &path)?;
        self.validate_locator(record, &locator)?;
        Ok(Some((locator, path)))
    }

    /// Validate a stored transcript locator without following a symlink or
    /// accepting a path outside its harness-owned root.
    pub(crate) fn validate_transcript(
        &self,
        record: &LocalHarnessRecord,
    ) -> Result<PathBuf, LocalHarnessResumeError> {
        ensure_supported_harness(record)?;
        let locator = record.transcript.as_ref().ok_or_else(|| {
            LocalHarnessResumeError::MissingTranscript {
                path: PathBuf::from("<not indexed>"),
            }
        })?;
        self.validate_locator(record, locator)
    }

    fn validate_locator(
        &self,
        record: &LocalHarnessRecord,
        locator: &TranscriptLocator,
    ) -> Result<PathBuf, LocalHarnessResumeError> {
        let expected_root = transcript_root(record.harness);
        if locator.root != expected_root {
            return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
                path: locator.relative_path.clone(),
            });
        }
        let root = match locator.root {
            TranscriptRoot::CodexSessions => &self.codex_sessions_root,
            TranscriptRoot::ClaudeProjects => &self.claude_projects_root,
        };
        let relative = safe_relative_path(&locator.relative_path)?;
        let root = canonical_root(root)?;
        let mut path = root.clone();
        let components = relative.components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
                    path: locator.relative_path.clone(),
                });
            };
            path.push(name);
            let metadata = fs::symlink_metadata(&path).map_err(|source| {
                if source.kind() == io::ErrorKind::NotFound {
                    LocalHarnessResumeError::MissingTranscript { path: path.clone() }
                } else {
                    io_error(path.clone(), source)
                }
            })?;
            if metadata.file_type().is_symlink() {
                return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
                    path: locator.relative_path.clone(),
                });
            }
            if index + 1 != components.len() && !metadata.is_dir() {
                return Err(LocalHarnessResumeError::MalformedTranscript {
                    path: path.clone(),
                    reason: "transcript parent is not a directory".to_owned(),
                });
            }
        }
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| io_error(path.clone(), source))?;
        if !metadata.is_file() {
            return Err(LocalHarnessResumeError::MalformedTranscript {
                path,
                reason: "transcript is not a regular file".to_owned(),
            });
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = options
            .open(&path)
            .map_err(|source| io_error(path.clone(), source))?;
        validate_transcript_jsonl(record.harness, record.harness_session_id, &path, &mut file)?;
        Ok(path)
    }

    fn ensure_index_dir(&self) -> Result<(), LocalHarnessResumeError> {
        if !self.index_dir.exists() {
            fs::create_dir_all(&self.index_dir)
                .map_err(|source| io_error(self.index_dir.clone(), source))?;
        }
        let metadata = fs::symlink_metadata(&self.index_dir)
            .map_err(|source| io_error(self.index_dir.clone(), source))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(LocalHarnessResumeError::Io {
                path: self.index_dir.clone(),
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "index path is not a directory",
                ),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.index_dir, fs::Permissions::from_mode(0o700))
                .map_err(|source| io_error(self.index_dir.clone(), source))?;
        }
        Ok(())
    }

    fn acquire_lock(&self, run_id: Uuid) -> Result<LockFile, LocalHarnessResumeError> {
        let path = self.index_dir.join(format!(".{run_id}.lock"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        }
        let file = options.open(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::AlreadyExists {
                LocalHarnessResumeError::Conflict {
                    run_id,
                    expected: 0,
                    actual: None,
                }
            } else {
                io_error(path.clone(), source)
            }
        })?;
        Ok(LockFile { path, _file: file })
    }

    fn write_atomically(
        &self,
        path: &Path,
        record: &LocalHarnessRecord,
        replace_existing: bool,
    ) -> Result<(), LocalHarnessResumeError> {
        let serialized = serde_json::to_vec_pretty(record)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let temp_path = parent.join(format!(
            ".{}.tmp-{}",
            path.file_name().unwrap().to_string_lossy(),
            Uuid::new_v4()
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options
                    .mode(0o600)
                    .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
            }
            let mut file = options
                .open(&temp_path)
                .map_err(|source| io_error(temp_path.clone(), source))?;
            file.write_all(&serialized)
                .and_then(|_| file.flush())
                .and_then(|_| file.sync_all())
                .map_err(|source| io_error(temp_path.clone(), source))?;
            drop(file);
            if !replace_existing {
                match fs::symlink_metadata(path) {
                    Ok(_) => {
                        return Err(LocalHarnessResumeError::Conflict {
                            run_id: record.run_id,
                            expected: 0,
                            actual: None,
                        });
                    }
                    Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                    Err(source) => return Err(io_error(path.to_path_buf(), source)),
                }
            }
            fs::rename(&temp_path, path).map_err(|source| io_error(path.to_path_buf(), source))?;
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }
}

struct LockFile {
    path: PathBuf,
    _file: File,
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn io_error(path: PathBuf, source: io::Error) -> LocalHarnessResumeError {
    LocalHarnessResumeError::Io { path, source }
}

fn ensure_supported_harness(record: &LocalHarnessRecord) -> Result<(), LocalHarnessResumeError> {
    match record.harness {
        Harness::Codex | Harness::Claude => Ok(()),
        harness => Err(LocalHarnessResumeError::UnsupportedHarness {
            run_id: record.run_id,
            harness,
        }),
    }
}

fn transcript_root(harness: Harness) -> TranscriptRoot {
    match harness {
        Harness::Codex => TranscriptRoot::CodexSessions,
        Harness::Claude => TranscriptRoot::ClaudeProjects,
        _ => TranscriptRoot::CodexSessions,
    }
}

fn codex_sessions_root() -> PathBuf {
    if let Some(path) = std::env::var_os(CODEX_HOME_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(path).join(CODEX_SESSIONS_DIR);
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(CODEX_HOME_DIR)
        .join(CODEX_SESSIONS_DIR)
}

fn claude_projects_root() -> PathBuf {
    let config = std::env::var_os(CLAUDE_CONFIG_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(CLAUDE_CONFIG_DIR)))
        .unwrap_or_default();
    config.join(CLAUDE_PROJECTS_DIR)
}

fn encode_claude_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy().replace(['/', '.'], "-")
}

fn canonical_root(root: &Path) -> Result<PathBuf, LocalHarnessResumeError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            LocalHarnessResumeError::MissingTranscript {
                path: root.to_path_buf(),
            }
        } else {
            io_error(root.to_path_buf(), source)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
            path: root.display().to_string(),
        });
    }
    fs::canonicalize(root).map_err(|source| io_error(root.to_path_buf(), source))
}

fn safe_relative_path(value: &str) -> Result<PathBuf, LocalHarnessResumeError> {
    let path = Path::new(value);
    if path.is_absolute() || value.is_empty() {
        return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
            path: value.to_owned(),
        });
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
                path: value.to_owned(),
            });
        }
    }
    Ok(path.to_path_buf())
}

fn locator_for_path(
    root: &Path,
    root_kind: TranscriptRoot,
    path: &Path,
) -> Result<TranscriptLocator, LocalHarnessResumeError> {
    let root = canonical_root(root)?;
    let path = fs::canonicalize(path).map_err(|source| io_error(path.to_path_buf(), source))?;
    let relative =
        path.strip_prefix(&root)
            .map_err(|_| LocalHarnessResumeError::UnsafeTranscriptPath {
                path: path.display().to_string(),
            })?;
    let relative_path = relative.to_string_lossy().replace('\\', "/");
    let _ = safe_relative_path(&relative_path)?;
    Ok(TranscriptLocator {
        root: root_kind,
        relative_path,
    })
}

fn find_codex_transcript(
    root: &Path,
    session_id: Uuid,
) -> Result<Option<PathBuf>, LocalHarnessResumeError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(root.to_path_buf(), source)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
            path: root.display().to_string(),
        });
    }
    let suffix = format!("-{session_id}.jsonl");
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|source| io_error(directory.clone(), source))?
            .flatten()
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|source| io_error(path.clone(), source))?;
            if metadata.file_type().is_symlink() {
                return Err(LocalHarnessResumeError::UnsafeTranscriptPath {
                    path: path.display().to_string(),
                });
            }
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with("rollout-") && name.ends_with(&suffix) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

fn validate_transcript_jsonl(
    harness: Harness,
    expected_session_id: Uuid,
    path: &Path,
    file: &mut File,
) -> Result<(), LocalHarnessResumeError> {
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|source| io_error(path.to_path_buf(), source))?;
    let mut entries = Vec::new();
    for (line_number, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(line).map_err(|error| {
            LocalHarnessResumeError::MalformedTranscript {
                path: path.to_path_buf(),
                reason: format!("line {}: {error}", line_number + 1),
            }
        })?;
        entries.push(value);
    }
    let found = match harness {
        Harness::Codex => entries.first().and_then(|entry| {
            (entry.get("type").and_then(Value::as_str) == Some("session_meta"))
                .then(|| entry.get("payload").and_then(|payload| payload.get("id")))
                .flatten()
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
        }),
        Harness::Claude => entries.iter().find_map(|entry| {
            ["sessionId", "session_id", "uuid"]
                .iter()
                .find_map(|key| entry.get(*key).and_then(Value::as_str))
                .and_then(|value| Uuid::parse_str(value).ok())
        }),
        _ => None,
    };
    if found != Some(expected_session_id) {
        return Err(LocalHarnessResumeError::TranscriptSessionMismatch {
            path: path.to_path_buf(),
            expected: expected_session_id,
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "local_resume_tests.rs"]
mod tests;
