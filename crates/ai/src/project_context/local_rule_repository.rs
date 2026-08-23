use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

/// The two filenames that are part of the local project-rule contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectRuleFile {
    Warp,
    Agents,
}

impl ProjectRuleFile {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Warp => "WARP.md",
            Self::Agents => "AGENTS.md",
        }
    }
}

/// A revision is deliberately stronger than a content hash. Including the
/// file metadata makes a save fail when another process replaced the file with
/// identical content but a different file identity or mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRevision {
    pub content_hash: [u8; 32],
    pub size: u64,
    pub modified: Option<SystemTime>,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
    #[cfg(unix)]
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRule {
    /// The canonical path that was validated and read.
    pub path: PathBuf,
    pub content: String,
    pub revision: RuleRevision,
    pub writable: bool,
}

#[derive(Debug, Error)]
pub enum LocalRuleError {
    #[error("rule path is invalid: {path}")]
    InvalidPath { path: PathBuf },
    #[error("rule path is not surfaced by the project context: {path}")]
    NotSurfaced { path: PathBuf },
    #[error("rule path is not a regular file: {path}")]
    NonRegular { path: PathBuf },
    #[error("rule path contains a symlink escape: {path}")]
    SymlinkEscape { path: PathBuf },
    #[error("rule file was not found: {path}")]
    NotFound { path: PathBuf },
    #[error("rule file changed while it was being edited: {path}")]
    Conflict {
        path: PathBuf,
        expected: RuleRevision,
        actual: Option<RuleRevision>,
    },
    #[error("managed rule root changed during the operation: {path}")]
    RootChanged { path: PathBuf },
    #[error("rule file already exists: {path}")]
    AlreadyExists { path: PathBuf },
    #[error("permission denied for rule path: {path}")]
    PermissionDenied { path: PathBuf },
    #[error("rule file is not valid UTF-8: {path}")]
    InvalidUtf8 { path: PathBuf },
    #[error("filesystem error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone)]
struct RootGuard {
    lexical: PathBuf,
    canonical: PathBuf,
}

/// File-backed CRUD for the rules surfaced by [`ProjectContextModel`].
///
/// The repository intentionally does not cache Markdown. The files remain the
/// source of truth; `surfaced_paths` only constrains which existing files an
/// editor may mutate. A successful create adds the exact canonical target to
/// that set so the subsequent edit/delete is still bound to the same surface.
#[derive(Debug, Default, Clone)]
pub struct LocalRuleRepository {
    surfaced_paths: BTreeSet<PathBuf>,
    project_roots: BTreeSet<PathBuf>,
    /// Test-only and embedding override. Production uses the exact
    /// `$HOME/.agents/AGENTS.md` target returned by [`global_target`].
    global_target_override: Option<PathBuf>,
    /// Project root for each surfaced project rule. Keeping this association
    /// avoids accepting a path merely because it happens to share a prefix.
    project_path_roots: HashMap<PathBuf, PathBuf>,
}

impl LocalRuleRepository {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a repository with paths supplied by tests or a host that has
    /// already indexed a context. The production UI should use
    /// [`Self::set_surfaced_paths`] on every context refresh instead.
    pub fn new_for_test<I, J>(global_paths: I, project_roots: J) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
        J: IntoIterator<Item = PathBuf>,
    {
        let mut repository = Self::default();
        let global_paths = global_paths.into_iter().collect::<Vec<_>>();
        if let Some(path) = global_paths.first() {
            repository.global_target_override = Some(path.clone());
        }
        repository.set_surfaced_paths(global_paths, project_roots);
        repository
    }

    /// Replace the repository's mutation allow-list with the exact paths
    /// currently surfaced by `ProjectContextModel`.
    pub fn set_surfaced_paths<I, J>(&mut self, global_paths: I, project_roots: J)
    where
        I: IntoIterator<Item = PathBuf>,
        J: IntoIterator<Item = PathBuf>,
    {
        self.surfaced_paths.clear();
        self.project_roots.clear();
        self.project_path_roots.clear();

        for path in global_paths {
            if let Ok(path) = self.canonical_surface_path(&path) {
                self.surfaced_paths.insert(path);
            }
        }

        for root in project_roots {
            let Ok(root) = canonical_directory(&root) else {
                continue;
            };
            self.project_roots.insert(root.clone());
            for file in [ProjectRuleFile::Warp, ProjectRuleFile::Agents] {
                let path = root.join(file.file_name());
                if is_regular_file(&path) {
                    self.surfaced_paths.insert(path.clone());
                    self.project_path_roots.insert(path, root.clone());
                }
            }
        }
    }

    /// Add one path from a context refresh without disturbing other rows.
    pub fn surface_path(
        &mut self,
        path: &Path,
        project_root: Option<&Path>,
    ) -> Result<(), LocalRuleError> {
        let path = self.canonical_surface_path(path)?;
        if let Some(root) = project_root {
            let root = canonical_directory(root)?;
            if !is_supported_project_path(&root, &path) {
                return Err(LocalRuleError::InvalidPath { path });
            }
            self.project_roots.insert(root.clone());
            self.project_path_roots.insert(path.clone(), root);
        }
        self.surfaced_paths.insert(path);
        Ok(())
    }

    pub fn surfaced_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.surfaced_paths.iter()
    }

    /// Read a surfaced rule and capture the revision used for a later CAS save.
    pub fn read(&self, path: &Path) -> Result<LocalRule, LocalRuleError> {
        let canonical = self.validate_surfaced_path(path)?;
        let (content, revision) = read_snapshot(&canonical)?;
        let writable = writable_file(&canonical);
        Ok(LocalRule {
            path: canonical,
            content,
            revision,
            writable,
        })
    }

    /// Create exactly `$HOME/.agents/AGENTS.md` (or the injected test target).
    pub fn create_global(&mut self, content: &str) -> Result<LocalRule, LocalRuleError> {
        let requested_target = self.global_target()?;
        let root = self.prepare_global_root(&requested_target)?;
        let target = root.canonical.join(file_name(&requested_target));
        let rule = self.atomic_write(&target, &root, None, content, true)?;
        self.surfaced_paths.insert(rule.path.clone());
        Ok(rule)
    }

    /// Create a project rule directly under an indexed project root.
    pub fn create_project(
        &mut self,
        project_root: &Path,
        file: ProjectRuleFile,
        content: &str,
    ) -> Result<LocalRule, LocalRuleError> {
        reject_untrusted_path(project_root)?;
        let root = canonical_directory(project_root)?;
        if !self.project_roots.contains(&root) {
            return Err(LocalRuleError::NotSurfaced { path: root.clone() });
        }
        let target = root.join(file.file_name());
        let root_guard = root_guard(&root)?;
        let rule = self.atomic_write(&target, &root_guard, None, content, true)?;
        self.surfaced_paths.insert(rule.path.clone());
        self.project_path_roots.insert(rule.path.clone(), root);
        Ok(rule)
    }

    /// Compare-and-swap update of a surfaced rule.
    pub fn update(
        &mut self,
        path: &Path,
        expected: &RuleRevision,
        content: &str,
    ) -> Result<LocalRule, LocalRuleError> {
        let canonical = self.validate_surfaced_path(path)?;
        let root = self.root_for_path(&canonical)?;
        let rule = self.atomic_write(&canonical, &root, Some(expected), content, false)?;
        self.surfaced_paths.insert(rule.path.clone());
        Ok(rule)
    }

    /// Compare-and-swap delete of one exact surfaced file. Parent directories
    /// are never removed.
    pub fn delete(&mut self, path: &Path, expected: &RuleRevision) -> Result<(), LocalRuleError> {
        let canonical = self.validate_surfaced_path(path)?;
        let root = self.root_for_path(&canonical)?;
        let (_, current) = snapshot(&canonical)?;
        if &current != expected {
            return Err(LocalRuleError::Conflict {
                path: canonical,
                expected: expected.clone(),
                actual: Some(current),
            });
        }
        ensure_root_unchanged(&root)?;
        remove_file(&canonical)?;
        // If a watcher or another process replaced the parent/root during the
        // delete, report the race instead of claiming a clean success.
        ensure_root_unchanged(&root)?;
        self.surfaced_paths.remove(&canonical);
        self.project_path_roots.remove(&canonical);
        Ok(())
    }

    fn atomic_write(
        &self,
        target: &Path,
        root: &RootGuard,
        expected: Option<&RuleRevision>,
        content: &str,
        creating: bool,
    ) -> Result<LocalRule, LocalRuleError> {
        ensure_root_unchanged(root)?;
        let old_mode = match snapshot(target) {
            Ok((_, revision)) => {
                if creating {
                    return Err(LocalRuleError::AlreadyExists {
                        path: target.to_path_buf(),
                    });
                }
                if let Some(expected) = expected
                    && expected != &revision
                {
                    return Err(LocalRuleError::Conflict {
                        path: target.to_path_buf(),
                        expected: expected.clone(),
                        actual: Some(revision),
                    });
                }
                Some(file_mode(target)?)
            }
            Err(LocalRuleError::NotFound { .. }) if creating => None,
            Err(error) => return Err(error),
        };

        let parent = target.parent().ok_or_else(|| LocalRuleError::InvalidPath {
            path: target.to_path_buf(),
        })?;
        validate_directory(parent)?;
        let temp_path = parent.join(format!(
            ".{}.warp-rule-{}",
            file_name(target),
            Uuid::new_v4()
        ));
        let write_result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(old_mode.unwrap_or(0o600));
            let mut file = options
                .open(&temp_path)
                .map_err(|error| map_io(&temp_path, error))?;
            file.write_all(content.as_bytes())
                .map_err(|error| map_io(&temp_path, error))?;
            if let Some(mode) = old_mode {
                #[cfg(unix)]
                file.set_permissions(fs::Permissions::from_mode(mode))
                    .map_err(|error| map_io(&temp_path, error))?;
            }
            file.sync_all().map_err(|error| map_io(&temp_path, error))?;
            drop(file);

            ensure_root_unchanged(root)?;
            let current = snapshot(target);
            match (creating, expected, current) {
                (true, _, Err(LocalRuleError::NotFound { .. })) => {}
                (true, _, Ok((_, actual))) => {
                    return Err(LocalRuleError::Conflict {
                        path: target.to_path_buf(),
                        expected: RuleRevision::empty(),
                        actual: Some(actual),
                    });
                }
                (false, Some(expected), Ok((_, actual))) if &actual != expected => {
                    return Err(LocalRuleError::Conflict {
                        path: target.to_path_buf(),
                        expected: expected.clone(),
                        actual: Some(actual),
                    });
                }
                (false, _, Err(LocalRuleError::NotFound { .. })) => {
                    return Err(LocalRuleError::Conflict {
                        path: target.to_path_buf(),
                        expected: expected.cloned().unwrap_or_else(RuleRevision::empty),
                        actual: None,
                    });
                }
                (_, _, Err(error)) => return Err(error),
                _ => {}
            }
            validate_directory(parent)?;
            rename(&temp_path, target)?;
            sync_directory(parent)?;
            Ok::<(), LocalRuleError>(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
            return Err(write_result.unwrap_err());
        }

        let (new_content, new_revision) = read_snapshot(target)?;
        Ok(LocalRule {
            path: target.to_path_buf(),
            content: new_content,
            revision: new_revision,
            writable: writable_file(target),
        })
    }

    fn global_target(&self) -> Result<PathBuf, LocalRuleError> {
        if let Some(path) = &self.global_target_override {
            reject_untrusted_path(path)?;
            return Ok(path.clone());
        }
        let home = dirs::home_dir().ok_or_else(|| LocalRuleError::InvalidPath {
            path: PathBuf::from("$HOME/.agents/AGENTS.md"),
        })?;
        Ok(home.join(".agents/AGENTS.md"))
    }

    fn prepare_global_root(&self, target: &Path) -> Result<RootGuard, LocalRuleError> {
        let home =
            target
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| LocalRuleError::InvalidPath {
                    path: target.to_path_buf(),
                })?;
        reject_untrusted_path(home)?;
        validate_directory(home)?;
        let parent = target.parent().unwrap();
        if parent.exists() {
            validate_directory(parent)?;
        } else {
            fs::create_dir(parent).map_err(|error| map_io(parent, error))?;
            validate_directory(parent)?;
        }
        root_guard(parent)
    }

    fn canonical_surface_path(&self, path: &Path) -> Result<PathBuf, LocalRuleError> {
        reject_untrusted_path(path)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(LocalRuleError::SymlinkEscape {
                    path: path.to_path_buf(),
                })
            }
            Ok(_) => canonical_existing(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let parent = path.parent().ok_or_else(|| LocalRuleError::InvalidPath {
                    path: path.to_path_buf(),
                })?;
                let parent = canonical_directory(parent)?;
                Ok(parent.join(file_name(path)))
            }
            Err(error) => Err(map_io(path, error)),
        }
    }

    fn validate_surfaced_path(&self, path: &Path) -> Result<PathBuf, LocalRuleError> {
        let canonical = self.canonical_surface_path(path)?;
        if !self.surfaced_paths.contains(&canonical) {
            return Err(LocalRuleError::NotSurfaced { path: canonical });
        }
        if self.is_global_target(&canonical) {
            return Ok(canonical);
        }
        if self.project_path_roots.contains_key(&canonical) {
            return Ok(canonical);
        }
        Err(LocalRuleError::NotSurfaced { path: canonical })
    }

    fn is_global_target(&self, path: &Path) -> bool {
        self.global_target()
            .ok()
            .and_then(|target| canonical_parent_target(&target).ok())
            .is_some_and(|target| target == path)
    }

    fn root_for_path(&self, path: &Path) -> Result<RootGuard, LocalRuleError> {
        if self.is_global_target(path) {
            return root_guard(path.parent().ok_or_else(|| LocalRuleError::InvalidPath {
                path: path.to_path_buf(),
            })?);
        }
        let root =
            self.project_path_roots
                .get(path)
                .ok_or_else(|| LocalRuleError::NotSurfaced {
                    path: path.to_path_buf(),
                })?;
        root_guard(root)
    }
}

impl RuleRevision {
    fn empty() -> Self {
        Self {
            content_hash: [0; 32],
            size: 0,
            modified: None,
            #[cfg(unix)]
            device: 0,
            #[cfg(unix)]
            inode: 0,
            #[cfg(unix)]
            mode: 0,
        }
    }
}

fn reject_untrusted_path(path: &Path) -> Result<(), LocalRuleError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(LocalRuleError::InvalidPath {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn canonical_existing(path: &Path) -> Result<PathBuf, LocalRuleError> {
    let canonical = fs::canonicalize(path).map_err(|error| map_io(path, error))?;
    Ok(canonical)
}

fn canonical_parent_target(path: &Path) -> Result<PathBuf, LocalRuleError> {
    let parent = path.parent().ok_or_else(|| LocalRuleError::InvalidPath {
        path: path.to_path_buf(),
    })?;
    let canonical_parent = canonical_directory(parent)?;
    Ok(canonical_parent.join(file_name(path)))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, LocalRuleError> {
    reject_untrusted_path(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(LocalRuleError::SymlinkEscape {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(LocalRuleError::NonRegular {
            path: path.to_path_buf(),
        });
    }
    let canonical = fs::canonicalize(path).map_err(|error| map_io(path, error))?;
    Ok(canonical)
}

fn validate_directory(path: &Path) -> Result<(), LocalRuleError> {
    let _ = canonical_directory(path)?;
    Ok(())
}

fn root_guard(path: &Path) -> Result<RootGuard, LocalRuleError> {
    Ok(RootGuard {
        lexical: path.to_path_buf(),
        canonical: canonical_directory(path)?,
    })
}

fn ensure_root_unchanged(root: &RootGuard) -> Result<(), LocalRuleError> {
    let current = canonical_directory(&root.lexical).map_err(|_| LocalRuleError::RootChanged {
        path: root.lexical.clone(),
    })?;
    if current != root.canonical {
        return Err(LocalRuleError::RootChanged {
            path: root.lexical.clone(),
        });
    }
    Ok(())
}

fn read_snapshot(path: &Path) -> Result<(String, RuleRevision), LocalRuleError> {
    let mut file = open_regular(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| map_io(path, error))?;
    let content = String::from_utf8(bytes.clone()).map_err(|_| LocalRuleError::InvalidUtf8 {
        path: path.to_path_buf(),
    })?;
    Ok((content, revision(path, &bytes)?))
}

fn snapshot(path: &Path) -> Result<(String, RuleRevision), LocalRuleError> {
    read_snapshot(path)
}

fn open_regular(path: &Path) -> Result<File, LocalRuleError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            LocalRuleError::NotFound {
                path: path.to_path_buf(),
            }
        } else {
            map_io(path, error)
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(LocalRuleError::SymlinkEscape {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(LocalRuleError::NonRegular {
            path: path.to_path_buf(),
        });
    }
    fs::canonicalize(path).map_err(|error| map_io(path, error))?;
    File::open(path).map_err(|error| map_io(path, error))
}

fn revision(path: &Path, bytes: &[u8]) -> Result<RuleRevision, LocalRuleError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let content_hash = hasher.finalize().into();
    Ok(RuleRevision {
        content_hash,
        size: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        mode: metadata.mode(),
    })
}

fn file_mode(path: &Path) -> Result<u32, LocalRuleError> {
    #[cfg(unix)]
    {
        Ok(fs::symlink_metadata(path)
            .map_err(|error| map_io(path, error))?
            .mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0)
    }
}

fn writable_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.file_type().is_symlink() && !metadata.permissions().readonly())
        .unwrap_or(false)
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rule")
}

fn is_supported_project_path(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root)
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("WARP.md" | "AGENTS.md")
        )
}

fn rename(from: &Path, to: &Path) -> Result<(), LocalRuleError> {
    fs::rename(from, to).map_err(|error| map_io(to, error))
}

fn remove_file(path: &Path) -> Result<(), LocalRuleError> {
    fs::remove_file(path).map_err(|error| map_io(path, error))
}

fn sync_directory(path: &Path) -> Result<(), LocalRuleError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| map_io(path, error))
}

fn map_io(path: &Path, source: io::Error) -> LocalRuleError {
    if source.kind() == io::ErrorKind::NotFound {
        LocalRuleError::NotFound {
            path: path.to_path_buf(),
        }
    } else if source.kind() == io::ErrorKind::PermissionDenied {
        LocalRuleError::PermissionDenied {
            path: path.to_path_buf(),
        }
    } else {
        LocalRuleError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
#[path = "local_rule_repository_tests.rs"]
mod tests;
