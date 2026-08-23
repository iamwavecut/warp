use std::collections::{BTreeSet, HashMap};
use std::fs;
#[cfg(not(unix))]
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(not(unix))]
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[cfg(unix)]
use nix::fcntl::{AtFlags, FlockArg, OFlag, flock, open, openat, renameat};
#[cfg(unix)]
use nix::sys::stat::{FileStat, Mode, SFlag, fchmod, fstat, fstatat, mkdirat};
#[cfg(unix)]
use nix::unistd::{LinkatFlags, UnlinkatFlags, close, fsync, linkat, read, unlinkat, write};
#[cfg(unix)]
use std::os::unix::io::RawFd;

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
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Debug, Clone)]
struct GlobalTarget {
    path: PathBuf,
    parent: RootGuard,
}

/// File-backed CRUD for the rules surfaced by [`ProjectContextModel`].
///
/// The repository intentionally does not cache Markdown. The files remain the
/// source of truth; `surfaced_paths` only constrains which existing or managed
/// missing files an editor may mutate. Unix mutations are anchored to an
/// opened, no-follow directory descriptor and protected by a directory lock.
/// This keeps path replacement from redirecting a write or delete to another
/// root between validation and the *at operation.
#[derive(Debug, Default, Clone)]
pub struct LocalRuleRepository {
    surfaced_paths: BTreeSet<PathBuf>,
    project_roots: BTreeSet<PathBuf>,
    /// Test-only and embedding override. Production uses the exact
    /// `$HOME/.agents/AGENTS.md` target returned by [`global_target`].
    global_target_override: Option<PathBuf>,
    global_target: Option<GlobalTarget>,
    /// Project root for each surfaced project rule. Keeping this association
    /// avoids accepting a path merely because it happens to share a prefix.
    project_path_roots: HashMap<PathBuf, PathBuf>,
    /// Identity captured when a project root was indexed. A root can be
    /// replaced at the same pathname between refreshes; mutations must remain
    /// bound to the directory that was actually surfaced.
    project_root_guards: HashMap<PathBuf, RootGuard>,
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
        self.project_root_guards.clear();
        self.global_target = self.global_target().ok().and_then(|target| {
            target
                .parent()
                .and_then(|parent| root_guard(parent).ok())
                .map(|parent| GlobalTarget {
                    path: parent.canonical.join(file_name(&target)),
                    parent,
                })
        });

        for path in global_paths {
            if let Ok(path) = self.canonical_surface_path(&path) {
                self.surfaced_paths.insert(path);
            }
        }
        // Keep the managed global target in the surface even when it is
        // unreadable or missing. The UI can then show an error row or offer
        // the exact create action without guessing from `$HOME` strings.
        if let Some(target) = &self.global_target {
            self.surfaced_paths.insert(target.path.clone());
        }

        for root in project_roots {
            let Ok(guard) = root_guard(&root) else {
                continue;
            };
            let root = guard.canonical.clone();
            self.project_roots.insert(root.clone());
            self.project_root_guards.insert(root.clone(), guard);
            // Both exact managed names stay surfaced even when one is absent,
            // unreadable, or non-regular. Precedence is resolved by the model,
            // while the Rules UI must expose both files for repair.
            for file in [ProjectRuleFile::Warp, ProjectRuleFile::Agents] {
                let path = root.join(file.file_name());
                self.surfaced_paths.insert(path.clone());
                self.project_path_roots.insert(path, root.clone());
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
            let guard = root_guard(root)?;
            let root = guard.canonical.clone();
            if !is_supported_project_path(&root, &path) {
                return Err(LocalRuleError::InvalidPath { path });
            }
            self.project_roots.insert(root.clone());
            self.project_root_guards.insert(root.clone(), guard);
            self.project_path_roots.insert(path.clone(), root);
        }
        self.surfaced_paths.insert(path);
        Ok(())
    }

    pub fn surfaced_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.surfaced_paths.iter()
    }

    pub fn is_global_path(&self, path: &Path) -> bool {
        let Some(target) = &self.global_target else {
            return false;
        };
        path == target.path && root_matches(&target.parent).unwrap_or(false)
    }

    pub fn project_roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.project_roots.iter()
    }

    pub fn managed_global_path(&self) -> Result<PathBuf, LocalRuleError> {
        if let Some(target) = &self.global_target {
            return Ok(target.path.clone());
        }
        self.global_target()
    }

    pub fn global_target_missing(&self) -> bool {
        let Ok(path) = self.managed_global_path() else {
            return false;
        };
        if let Some(target) = &self.global_target {
            return matches!(
                self.read(&target.path),
                Err(LocalRuleError::NotFound { .. })
            );
        }
        matches!(fs::symlink_metadata(path), Err(error) if error.kind() == io::ErrorKind::NotFound)
    }

    pub fn project_rule_path(
        &self,
        project_root: &Path,
        file: ProjectRuleFile,
    ) -> Result<PathBuf, LocalRuleError> {
        let root = canonical_directory(project_root)?;
        if !self.project_roots.contains(&root) {
            return Err(LocalRuleError::NotSurfaced { path: root });
        }
        let guard = self
            .project_root_guards
            .get(&root)
            .ok_or_else(|| LocalRuleError::NotSurfaced { path: root.clone() })?;
        ensure_root_unchanged(guard)?;
        Ok(root.join(file.file_name()))
    }

    /// Read a surfaced rule and capture the revision used for a later CAS save.
    pub fn read(&self, path: &Path) -> Result<LocalRule, LocalRuleError> {
        let canonical = self.validate_surfaced_path(path)?;
        let root = self.root_for_path(&canonical)?;
        let directory = open_locked_root(&root, FlockArg::LockShared)?;
        let (content, revision) = read_snapshot_at(&directory, file_name(&canonical), &canonical)?;
        Ok(LocalRule {
            path: canonical,
            content,
            writable: revision_is_writable(&revision),
            revision,
        })
    }

    /// Create exactly `$HOME/.agents/AGENTS.md` (or the injected test target).
    pub fn create_global(&mut self, content: &str) -> Result<LocalRule, LocalRuleError> {
        let requested_target = self.global_target()?;
        let root = self.prepare_global_root(&requested_target)?;
        let target = root.canonical.join(file_name(&requested_target));
        let rule = self.atomic_write(&target, &root, None, content, true)?;
        self.global_target = Some(GlobalTarget {
            path: target.clone(),
            parent: root.clone(),
        });
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
        let root_guard = self
            .project_root_guards
            .get(&root)
            .cloned()
            .ok_or_else(|| LocalRuleError::NotSurfaced { path: root.clone() })?;
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
        #[cfg(unix)]
        {
            let directory = open_locked_root(&root, FlockArg::LockExclusive)?;
            let Some((_, current)) =
                snapshot_at_optional(&directory, file_name(&canonical), &canonical)?
            else {
                return Err(LocalRuleError::Conflict {
                    path: canonical,
                    expected: expected.clone(),
                    actual: None,
                });
            };
            if &current != expected {
                return Err(LocalRuleError::Conflict {
                    path: canonical,
                    expected: expected.clone(),
                    actual: Some(current),
                });
            }
            ensure_root_unchanged(&root)?;
            let backup = backup_name(file_name(&canonical));
            renameat(
                Some(directory.fd()),
                file_name(&canonical),
                Some(directory.fd()),
                backup.as_str(),
            )
            .map_err(|error| map_nix(&canonical, error))?;
            if let Err(error) = sync_fd(&directory, &canonical) {
                if restore_backup(&directory, file_name(&canonical), &backup).is_ok() {
                    let _ = sync_fd(&directory, &canonical);
                    return Err(error);
                }
                // The target pathname is already absent and the old inode is
                // retained under the unique backup name. Treat the delete as
                // successful rather than reporting an error after publishing
                // a changed directory state; the backup remains recoverable.
                return Ok(());
            }
            // Once the deletion is durably published, cleanup cannot turn a
            // successful delete into an error. A leftover backup is recoverable.
            let _ = unlinkat(
                Some(directory.fd()),
                backup.as_str(),
                UnlinkatFlags::NoRemoveDir,
            );
            let _ = sync_fd(&directory, &canonical);
        }
        #[cfg(not(unix))]
        {
            let (_, current) = snapshot(&canonical)?;
            if &current != expected {
                return Err(LocalRuleError::Conflict {
                    path: canonical,
                    expected: expected.clone(),
                    actual: Some(current),
                });
            }
            remove_file(&canonical)?;
        }
        // Keep the managed pathname surfaced after deletion. A missing target
        // is still an allowed exact create target, so the Rules UI can expose
        // Add again without guessing a different project root or filename.
        Ok(())
    }

    #[cfg(unix)]
    fn atomic_write(
        &self,
        target: &Path,
        root: &RootGuard,
        expected: Option<&RuleRevision>,
        content: &str,
        creating: bool,
    ) -> Result<LocalRule, LocalRuleError> {
        let directory = open_locked_root(root, FlockArg::LockExclusive)?;
        let target_name = file_name(target);
        let existing = snapshot_at_optional(&directory, target_name, target)?;
        let old_revision = match existing {
            Some((_, _revision)) if creating => {
                return Err(LocalRuleError::AlreadyExists {
                    path: target.to_path_buf(),
                });
            }
            Some((_, revision)) => {
                if let Some(expected) = expected
                    && expected != &revision
                {
                    return Err(LocalRuleError::Conflict {
                        path: target.to_path_buf(),
                        expected: expected.clone(),
                        actual: Some(revision),
                    });
                }
                Some(revision)
            }
            None if creating => None,
            None => {
                return Err(LocalRuleError::Conflict {
                    path: target.to_path_buf(),
                    expected: expected.cloned().unwrap_or_else(RuleRevision::empty),
                    actual: None,
                });
            }
        };
        ensure_root_unchanged(root)?;

        // Re-check the target through the same locked directory immediately
        // before publishing. No path-string check is used for the CAS decision.
        let current = snapshot_at_optional(&directory, target_name, target)?;
        match (&old_revision, current) {
            (None, None) => {}
            (Some(old), Some((_, current))) if &current == old => {}
            (Some(old), Some((_, current))) => {
                return Err(LocalRuleError::Conflict {
                    path: target.to_path_buf(),
                    expected: expected.cloned().unwrap_or_else(|| old.clone()),
                    actual: Some(current),
                });
            }
            (None, Some((_, current))) => {
                return Err(LocalRuleError::Conflict {
                    path: target.to_path_buf(),
                    expected: RuleRevision::empty(),
                    actual: Some(current),
                });
            }
            (Some(old), None) => {
                return Err(LocalRuleError::Conflict {
                    path: target.to_path_buf(),
                    expected: expected.cloned().unwrap_or_else(|| old.clone()),
                    actual: None,
                });
            }
        }

        let temp = temp_name(target_name);
        let mode = old_revision
            .as_ref()
            .map(|revision| revision.mode)
            .unwrap_or(0o600);
        let new_revision = write_temp(&directory, &temp, content, mode, target)?;

        if creating {
            match linkat(
                Some(directory.fd()),
                temp.as_str(),
                Some(directory.fd()),
                target_name,
                LinkatFlags::NoSymlinkFollow,
            ) {
                Ok(()) => {
                    // The hard link is the create publication. Removing the
                    // temporary name cannot invalidate the target inode.
                    cleanup_entry(&directory, &temp);
                    if let Err(error) = sync_fd(&directory, target) {
                        if remove_entry(&directory, target_name).is_ok() {
                            let _ = sync_fd(&directory, target);
                            return Err(error);
                        }
                        // Keep the published file visible if rollback races;
                        // returning an error after that publication would be
                        // misleading and would lose the recoverable result.
                        return Ok(LocalRule {
                            path: target.to_path_buf(),
                            content: content.to_string(),
                            revision: new_revision,
                            writable: mode & 0o222 != 0,
                        });
                    }
                    let _ = sync_fd(&directory, target);
                    return Ok(LocalRule {
                        path: target.to_path_buf(),
                        content: content.to_string(),
                        revision: new_revision,
                        writable: mode & 0o222 != 0,
                    });
                }
                Err(error) if error == nix::errno::Errno::EEXIST => {
                    cleanup_entry(&directory, &temp);
                    return Err(LocalRuleError::AlreadyExists {
                        path: target.to_path_buf(),
                    });
                }
                Err(error) => {
                    cleanup_entry(&directory, &temp);
                    return Err(map_nix(target, error));
                }
            }
        }

        let backup = old_revision.as_ref().map(|_| backup_name(target_name));

        if let Some(backup) = &backup {
            if let Err(error) = renameat(
                Some(directory.fd()),
                target_name,
                Some(directory.fd()),
                backup.as_str(),
            ) {
                cleanup_entry(&directory, &temp);
                return Err(map_nix(target, error));
            }
        }

        if let Err(error) = renameat(
            Some(directory.fd()),
            temp.as_str(),
            Some(directory.fd()),
            target_name,
        ) {
            if let Some(backup) = &backup {
                let _ = restore_backup(&directory, target_name, backup);
            }
            cleanup_entry(&directory, &temp);
            return Err(map_nix(target, error));
        }

        if let Err(error) = sync_fd(&directory, target) {
            let rollback = if let Some(backup) = &backup {
                restore_backup(&directory, target_name, backup)
            } else {
                remove_entry(&directory, target_name)
            };
            if rollback.is_ok() {
                let _ = sync_fd(&directory, target);
                return Err(error);
            }
            // Keep the published version visible and retain the old version
            // in its same-directory backup if rollback itself raced with an
            // external filesystem change. Returning success here avoids
            // claiming failure after a visible rename; the next read still
            // has a recoverable backup available to an operator.
            return Ok(LocalRule {
                path: target.to_path_buf(),
                content: content.to_string(),
                revision: new_revision,
                writable: mode & 0o222 != 0,
            });
        }

        // Publishing succeeded and was synced. Never return a cleanup error
        // after that point: a stale backup is recoverable and old content is
        // no longer the visible version.
        if let Some(backup) = &backup {
            cleanup_entry(&directory, backup);
        }
        let _ = sync_fd(&directory, target);
        Ok(LocalRule {
            path: target.to_path_buf(),
            content: content.to_string(),
            revision: new_revision,
            writable: mode & 0o222 != 0,
        })
    }

    #[cfg(not(unix))]
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
                None
            }
            Err(LocalRuleError::NotFound { .. }) if creating => None,
            Err(error) => return Err(error),
        };
        let parent = target.parent().ok_or_else(|| LocalRuleError::InvalidPath {
            path: target.to_path_buf(),
        })?;
        let temp_path = parent.join(format!(
            ".{}.warp-rule-{}",
            file_name(target),
            Uuid::new_v4()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| map_io(&temp_path, error))?;
        file.write_all(content.as_bytes())
            .map_err(|error| map_io(&temp_path, error))?;
        file.sync_all().map_err(|error| map_io(&temp_path, error))?;
        drop(file);
        fs::rename(&temp_path, target).map_err(|error| map_io(target, error))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| map_io(parent, error))?;
        let (new_content, new_revision) = read_snapshot(target)?;
        Ok(LocalRule {
            path: target.to_path_buf(),
            content: new_content,
            revision: new_revision,
            writable: old_mode.is_some(),
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
        #[cfg(unix)]
        {
            let home_fd = open_directory_nofollow(home)?;
            let directory_name = target.parent().and_then(Path::file_name).ok_or_else(|| {
                LocalRuleError::InvalidPath {
                    path: target.to_path_buf(),
                }
            })?;
            match mkdirat(
                home_fd.fd(),
                directory_name,
                Mode::from_bits_truncate(0o700),
            ) {
                Ok(()) => {}
                Err(error) if error == nix::errno::Errno::EEXIST => {}
                Err(error) => return Err(map_nix(target, error)),
            }
            return root_guard(target.parent().unwrap());
        }
        #[cfg(not(unix))]
        {
            validate_directory(home)?;
            let parent = target.parent().unwrap();
            if !parent.exists() {
                fs::create_dir(parent).map_err(|error| map_io(parent, error))?;
            }
            root_guard(parent)
        }
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
        if self.is_global_path(&canonical) || self.project_path_roots.contains_key(&canonical) {
            return Ok(canonical);
        }
        Err(LocalRuleError::NotSurfaced { path: canonical })
    }

    fn root_for_path(&self, path: &Path) -> Result<RootGuard, LocalRuleError> {
        if let Some(global) = &self.global_target
            && path == global.path
        {
            return Ok(global.parent.clone());
        }
        let root =
            self.project_path_roots
                .get(path)
                .ok_or_else(|| LocalRuleError::NotSurfaced {
                    path: path.to_path_buf(),
                })?;
        self.project_root_guards
            .get(root)
            .cloned()
            .ok_or_else(|| LocalRuleError::RootChanged {
                path: root.to_path_buf(),
            })
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

#[cfg(unix)]
#[derive(Debug)]
struct FdGuard {
    fd: RawFd,
}

#[cfg(unix)]
impl FdGuard {
    fn new(fd: RawFd) -> Self {
        Self { fd }
    }

    fn fd(&self) -> RawFd {
        self.fd
    }
}

#[cfg(unix)]
impl Drop for FdGuard {
    fn drop(&mut self) {
        let _ = close(self.fd);
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct DirectoryLock {
    fd: FdGuard,
}

#[cfg(unix)]
impl DirectoryLock {
    fn fd(&self) -> RawFd {
        self.fd.fd()
    }
}

#[cfg(unix)]
fn open_directory_nofollow(path: &Path) -> Result<FdGuard, LocalRuleError> {
    let canonical = canonical_directory(path)?;
    open_directory_nofollow_raw(&canonical)
}

#[cfg(unix)]
fn open_directory_nofollow_raw(path: &Path) -> Result<FdGuard, LocalRuleError> {
    reject_untrusted_path(path)?;
    let mut directory = FdGuard::new(
        open(
            Path::new("/"),
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|error| map_nix(path, error))?,
    );
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let next = openat(
            directory.fd(),
            name,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|error| map_nix(path, error))?;
        directory = FdGuard::new(next);
    }
    Ok(directory)
}

#[cfg(unix)]
fn stat_identity(stat: &FileStat) -> (u64, u64) {
    (stat.st_dev as u64, stat.st_ino as u64)
}

#[cfg(unix)]
fn root_guard(path: &Path) -> Result<RootGuard, LocalRuleError> {
    let canonical = canonical_directory(path)?;
    let directory = open_directory_nofollow_raw(&canonical)?;
    let stat = fstat(directory.fd()).map_err(|error| map_nix(path, error))?;
    Ok(RootGuard {
        lexical: canonical.clone(),
        canonical,
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    })
}

#[cfg(not(unix))]
fn root_guard(path: &Path) -> Result<RootGuard, LocalRuleError> {
    Ok(RootGuard {
        lexical: path.to_path_buf(),
        canonical: canonical_directory(path)?,
    })
}

#[cfg(unix)]
fn root_matches(root: &RootGuard) -> Result<bool, LocalRuleError> {
    let directory = open_directory_nofollow_raw(&root.canonical)?;
    let stat = fstat(directory.fd()).map_err(|error| map_nix(&root.lexical, error))?;
    Ok(stat_identity(&stat) == (root.device, root.inode))
}

#[cfg(not(unix))]
fn root_matches(root: &RootGuard) -> Result<bool, LocalRuleError> {
    Ok(canonical_directory(&root.lexical)? == root.canonical)
}

#[cfg(unix)]
fn ensure_root_unchanged(root: &RootGuard) -> Result<(), LocalRuleError> {
    if root_matches(root).map_err(|_| LocalRuleError::RootChanged {
        path: root.lexical.clone(),
    })? {
        Ok(())
    } else {
        Err(LocalRuleError::RootChanged {
            path: root.lexical.clone(),
        })
    }
}

#[cfg(not(unix))]
fn ensure_root_unchanged(root: &RootGuard) -> Result<(), LocalRuleError> {
    if canonical_directory(&root.lexical)? == root.canonical {
        Ok(())
    } else {
        Err(LocalRuleError::RootChanged {
            path: root.lexical.clone(),
        })
    }
}

#[cfg(unix)]
fn open_locked_root(root: &RootGuard, lock: FlockArg) -> Result<DirectoryLock, LocalRuleError> {
    let directory = open_directory_nofollow_raw(&root.canonical)?;
    let stat = fstat(directory.fd()).map_err(|error| map_nix(&root.lexical, error))?;
    if stat_identity(&stat) != (root.device, root.inode) {
        return Err(LocalRuleError::RootChanged {
            path: root.lexical.clone(),
        });
    }
    flock(directory.fd(), lock).map_err(|error| map_nix(&root.lexical, error))?;
    Ok(DirectoryLock { fd: directory })
}

#[cfg(unix)]
fn open_at_nofollow(
    directory: &DirectoryLock,
    name: &str,
    path: &Path,
) -> Result<FdGuard, LocalRuleError> {
    match openat(
        directory.fd(),
        name,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(FdGuard::new(fd)),
        Err(error) if error == nix::errno::Errno::ELOOP => Err(LocalRuleError::SymlinkEscape {
            path: path.to_path_buf(),
        }),
        Err(error) => Err(map_nix(path, error)),
    }
}

#[cfg(unix)]
fn stat_at(
    directory: &DirectoryLock,
    name: &str,
    path: &Path,
) -> Result<Option<FileStat>, LocalRuleError> {
    match fstatat(directory.fd(), name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let file_type = SFlag::from_bits_truncate(stat.st_mode);
            if file_type.contains(SFlag::S_IFLNK) {
                return Err(LocalRuleError::SymlinkEscape {
                    path: path.to_path_buf(),
                });
            }
            Ok(Some(stat))
        }
        Err(error) if error == nix::errno::Errno::ENOENT => Ok(None),
        Err(error) => Err(map_nix(path, error)),
    }
}

#[cfg(unix)]
fn read_snapshot_at(
    directory: &DirectoryLock,
    name: &str,
    path: &Path,
) -> Result<(String, RuleRevision), LocalRuleError> {
    let file = open_at_nofollow(directory, name, path)?;
    let stat = fstat(file.fd()).map_err(|error| map_nix(path, error))?;
    if !SFlag::from_bits_truncate(stat.st_mode).contains(SFlag::S_IFREG) {
        return Err(LocalRuleError::NonRegular {
            path: path.to_path_buf(),
        });
    }
    let bytes = read_all(file.fd(), path)?;
    let content = String::from_utf8(bytes.clone()).map_err(|_| LocalRuleError::InvalidUtf8 {
        path: path.to_path_buf(),
    })?;
    Ok((content, revision_from_stat(&stat, &bytes)))
}

#[cfg(unix)]
fn snapshot_at_optional(
    directory: &DirectoryLock,
    name: &str,
    path: &Path,
) -> Result<Option<(String, RuleRevision)>, LocalRuleError> {
    if stat_at(directory, name, path)?.is_none() {
        return Ok(None);
    }
    read_snapshot_at(directory, name, path).map(Some)
}

#[cfg(unix)]
fn write_temp(
    directory: &DirectoryLock,
    name: &str,
    content: &str,
    mode: u32,
    target: &Path,
) -> Result<RuleRevision, LocalRuleError> {
    let file = openat(
        directory.fd(),
        name,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(mode as _),
    )
    .map_err(|error| map_nix(target, error))?;
    let file = FdGuard::new(file);
    if let Err(error) = write_all(file.fd(), content.as_bytes(), target) {
        cleanup_entry(directory, name);
        return Err(error);
    }
    if let Err(error) = fchmod(file.fd(), Mode::from_bits_truncate(mode as _)) {
        cleanup_entry(directory, name);
        return Err(map_nix(target, error));
    }
    if let Err(error) = fsync(file.fd()) {
        cleanup_entry(directory, name);
        return Err(map_nix(target, error));
    }
    let stat = match fstat(file.fd()) {
        Ok(stat) => stat,
        Err(error) => {
            cleanup_entry(directory, name);
            return Err(map_nix(target, error));
        }
    };
    Ok(revision_from_stat(&stat, content.as_bytes()))
}

#[cfg(unix)]
fn read_all(fd: RawFd, path: &Path) -> Result<Vec<u8>, LocalRuleError> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match read(fd, &mut buffer) {
            Ok(0) => return Ok(bytes),
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error == nix::errno::Errno::EINTR => {}
            Err(error) => return Err(map_nix(path, error)),
        }
    }
}

#[cfg(unix)]
fn write_all(fd: RawFd, bytes: &[u8], path: &Path) -> Result<(), LocalRuleError> {
    let mut offset = 0;
    while offset < bytes.len() {
        match write(fd, &bytes[offset..]) {
            Ok(0) => {
                return Err(LocalRuleError::Io {
                    path: path.to_path_buf(),
                    source: io::Error::new(io::ErrorKind::WriteZero, "short rule write"),
                });
            }
            Ok(count) => offset += count,
            Err(error) if error == nix::errno::Errno::EINTR => {}
            Err(error) => return Err(map_nix(path, error)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_fd(directory: &DirectoryLock, path: &Path) -> Result<(), LocalRuleError> {
    fsync(directory.fd()).map_err(|error| map_nix(path, error))
}

#[cfg(unix)]
fn restore_backup(
    directory: &DirectoryLock,
    target: &str,
    backup: &str,
) -> Result<(), LocalRuleError> {
    renameat(Some(directory.fd()), backup, Some(directory.fd()), target)
        .map_err(|error| map_nix(Path::new(target), error))
}

#[cfg(unix)]
fn remove_entry(directory: &DirectoryLock, name: &str) -> Result<(), LocalRuleError> {
    unlinkat(Some(directory.fd()), name, UnlinkatFlags::NoRemoveDir)
        .map_err(|error| map_nix(Path::new(name), error))
}

#[cfg(unix)]
fn cleanup_entry(directory: &DirectoryLock, name: &str) {
    let _ = unlinkat(Some(directory.fd()), name, UnlinkatFlags::NoRemoveDir);
}

#[cfg(unix)]
fn temp_name(target: &str) -> String {
    format!(".{}.warp-rule-{}", target, Uuid::new_v4())
}

#[cfg(unix)]
fn backup_name(target: &str) -> String {
    format!(".{}.warp-rule-backup-{}", target, Uuid::new_v4())
}

#[cfg(unix)]
fn revision_from_stat(stat: &FileStat, bytes: &[u8]) -> RuleRevision {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    RuleRevision {
        content_hash: hasher.finalize().into(),
        size: stat.st_size as u64,
        modified: None,
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
        mode: stat.st_mode as u32,
    }
}

#[cfg(unix)]
fn revision_is_writable(revision: &RuleRevision) -> bool {
    revision.mode & 0o222 != 0
}

#[cfg(not(unix))]
fn canonical_existing(path: &Path) -> Result<PathBuf, LocalRuleError> {
    fs::canonicalize(path).map_err(|error| map_io(path, error))
}

#[cfg(unix)]
fn canonical_existing(path: &Path) -> Result<PathBuf, LocalRuleError> {
    let parent = path.parent().ok_or_else(|| LocalRuleError::InvalidPath {
        path: path.to_path_buf(),
    })?;
    let parent = canonical_directory(parent)?;
    let directory = open_directory_nofollow_raw(&parent)?;
    let directory = DirectoryLock { fd: directory };
    let Some(_stat) = stat_at(&directory, file_name(path), path)? else {
        return Err(LocalRuleError::NotFound {
            path: path.to_path_buf(),
        });
    };
    Ok(parent.join(file_name(path)))
}

#[cfg(not(unix))]
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
    fs::canonicalize(path).map_err(|error| map_io(path, error))
}

#[cfg(unix)]
fn canonical_directory(path: &Path) -> Result<PathBuf, LocalRuleError> {
    reject_untrusted_path(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(LocalRuleError::SymlinkEscape {
            path: path.to_path_buf(),
        });
    }
    let canonical = fs::canonicalize(path).map_err(|error| map_io(path, error))?;
    let _ = open_directory_nofollow_raw(&canonical)?;
    Ok(canonical)
}

#[cfg(not(unix))]
fn validate_directory(path: &Path) -> Result<(), LocalRuleError> {
    let _ = canonical_directory(path)?;
    Ok(())
}

#[cfg(not(unix))]
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

#[cfg(not(unix))]
fn snapshot(path: &Path) -> Result<(String, RuleRevision), LocalRuleError> {
    read_snapshot(path)
}

#[cfg(not(unix))]
fn open_regular(path: &Path) -> Result<File, LocalRuleError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
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
    File::open(path).map_err(|error| map_io(path, error))
}

#[cfg(not(unix))]
fn revision(path: &Path, bytes: &[u8]) -> Result<RuleRevision, LocalRuleError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(RuleRevision {
        content_hash: hasher.finalize().into(),
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[cfg(not(unix))]
fn writable_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.file_type().is_symlink() && !metadata.permissions().readonly())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn remove_file(path: &Path) -> Result<(), LocalRuleError> {
    fs::remove_file(path).map_err(|error| map_io(path, error))
}

fn is_supported_project_path(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root)
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("WARP.md" | "AGENTS.md")
        )
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rule")
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

#[cfg(unix)]
fn map_nix(path: &Path, source: nix::Error) -> LocalRuleError {
    map_io(path, io::Error::from_raw_os_error(source as i32))
}

#[cfg(test)]
#[path = "local_rule_repository_tests.rs"]
mod tests;
