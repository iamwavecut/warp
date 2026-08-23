use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use uuid::Uuid;

use super::workflow::Workflow;

/// Directory below the user's local workflow directory that is owned by the
/// saved-prompt editor.
pub const LOCAL_SAVED_PROMPTS_DIR: &str = "local-prompts";

const YAML_EXTENSION: &str = "yaml";
const ATOMIC_TEMP_PREFIX: &str = ".";
const ATOMIC_TEMP_MARKER: &str = ".tmp-";

#[derive(Debug, thiserror::Error)]
pub enum LocalSavedPromptRepositoryError {
    #[error("saved prompt must be an Agent Mode workflow")]
    NotAgentMode,
    #[error("saved prompt {id} does not exist")]
    Missing { id: Uuid },
    #[error("saved prompt name '{name}' is ambiguous")]
    AmbiguousName { name: String },
    #[error("saved prompt selector '{selector}' was not found")]
    NotFound { selector: String },
    #[error("saved prompt file {path} contains multiple YAML documents")]
    MultipleDocuments { path: PathBuf },
    #[error("saved prompt file {path} is not a regular managed file")]
    NotManaged { path: PathBuf },
    #[error("could not parse saved prompt file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("could not access saved prompt file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not serialize saved prompt: {0}")]
    Serialize(#[from] serde_yaml::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSavedPrompt {
    id: Uuid,
    workflow: Workflow,
    path: PathBuf,
}

impl LocalSavedPrompt {
    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn workflow(&self) -> &Workflow {
        &self.workflow
    }

    pub fn into_workflow(self) -> Workflow {
        self.workflow
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// CRUD for editor-owned local Agent Mode workflows.
///
/// The repository intentionally has no filename input API. A UUID is the
/// stable identity and the only value used to derive a managed path; display
/// names are data inside the YAML document and can change freely.
#[derive(Clone, Debug)]
pub struct LocalSavedPromptRepository {
    workflows_dir: PathBuf,
}

impl LocalSavedPromptRepository {
    pub fn new(workflows_dir: impl AsRef<Path>) -> Self {
        Self {
            workflows_dir: workflows_dir.as_ref().to_path_buf(),
        }
    }

    pub fn for_user() -> Self {
        Self::new(crate::user_config::workflows_dir())
    }

    pub fn workflows_dir(&self) -> &Path {
        &self.workflows_dir
    }

    pub fn managed_dir(&self) -> PathBuf {
        self.workflows_dir.join(LOCAL_SAVED_PROMPTS_DIR)
    }

    pub fn path_for_id(&self, id: Uuid) -> PathBuf {
        self.managed_dir().join(format!("{id}.{YAML_EXTENSION}"))
    }

    pub fn create(
        &self,
        workflow: Workflow,
    ) -> Result<LocalSavedPrompt, LocalSavedPromptRepositoryError> {
        ensure_agent_mode(&workflow)?;
        self.ensure_managed_dir()?;

        // UUID collisions are exceptionally unlikely, but a collision must
        // never overwrite a user's existing prompt.
        for _ in 0..16 {
            let id = Uuid::new_v4();
            let path = self.path_for_id(id);
            match fs::symlink_metadata(&path) {
                Ok(_) => continue,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(LocalSavedPromptRepositoryError::Io { path, source });
                }
            }
            self.write_atomically(&path, &workflow, false)?;
            return Ok(LocalSavedPrompt { id, workflow, path });
        }

        Err(LocalSavedPromptRepositoryError::Io {
            path: self.managed_dir(),
            source: io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate a unique saved prompt id",
            ),
        })
    }

    pub fn get(
        &self,
        id: Uuid,
    ) -> Result<Option<LocalSavedPrompt>, LocalSavedPromptRepositoryError> {
        let path = self.path_for_id(id);
        match fs::symlink_metadata(&path) {
            Ok(_) => self.read_path(id, &path).map(Some),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(LocalSavedPromptRepositoryError::Io { path, source }),
        }
    }

    pub fn update(
        &self,
        id: Uuid,
        workflow: Workflow,
    ) -> Result<LocalSavedPrompt, LocalSavedPromptRepositoryError> {
        ensure_agent_mode(&workflow)?;
        let path = self.path_for_id(id);
        // Read and validate the prior file before creating a replacement. This
        // keeps malformed or multi-document files read-only and guarantees a
        // failed update cannot silently turn them into managed files.
        if self.get(id)?.is_none() {
            return Err(LocalSavedPromptRepositoryError::Missing { id });
        }
        self.write_atomically(&path, &workflow, true)?;
        Ok(LocalSavedPrompt { id, workflow, path })
    }

    pub fn delete(&self, id: Uuid) -> Result<(), LocalSavedPromptRepositoryError> {
        let path = self.path_for_id(id);
        // Validation is deliberate: an unparseable or multi-document file is
        // never deleted automatically, even if its basename is a UUID.
        if self.get(id)?.is_none() {
            return Err(LocalSavedPromptRepositoryError::Missing { id });
        }
        fs::remove_file(&path)
            .map_err(|source| LocalSavedPromptRepositoryError::Io { path, source })
    }

    pub fn list(&self) -> Result<Vec<LocalSavedPrompt>, LocalSavedPromptRepositoryError> {
        let dir = self.managed_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(LocalSavedPromptRepositoryError::Io { path: dir, source });
            }
        };

        let mut prompts = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| LocalSavedPromptRepositoryError::Io {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if !is_yaml_path(&path) {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| Uuid::parse_str(stem).ok())
            else {
                // Files not named with a UUID are unmanaged and remain part
                // of the ordinary read-only WarpConfig discovery path.
                continue;
            };
            prompts.push(self.read_path(id, &path)?);
        }
        prompts.sort_by_key(|prompt| (prompt.workflow.name().to_owned(), prompt.id));
        Ok(prompts)
    }

    pub fn resolve(
        &self,
        selector: &str,
    ) -> Result<LocalSavedPrompt, LocalSavedPromptRepositoryError> {
        if let Ok(id) = Uuid::parse_str(selector) {
            if let Some(prompt) = self.get(id)? {
                return Ok(prompt);
            }
        }

        let matches = self
            .list()?
            .into_iter()
            .filter(|prompt| prompt.workflow.name() == selector)
            .collect::<Vec<_>>();
        match matches.len() {
            1 => Ok(matches.into_iter().next().expect("one match")),
            0 => Err(LocalSavedPromptRepositoryError::NotFound {
                selector: selector.to_owned(),
            }),
            _ => Err(LocalSavedPromptRepositoryError::AmbiguousName {
                name: selector.to_owned(),
            }),
        }
    }

    fn ensure_managed_dir(&self) -> Result<(), LocalSavedPromptRepositoryError> {
        fs::create_dir_all(self.managed_dir()).map_err(|source| {
            LocalSavedPromptRepositoryError::Io {
                path: self.managed_dir(),
                source,
            }
        })
    }

    fn read_path(
        &self,
        id: Uuid,
        path: &Path,
    ) -> Result<LocalSavedPrompt, LocalSavedPromptRepositoryError> {
        let metadata =
            fs::symlink_metadata(path).map_err(|source| LocalSavedPromptRepositoryError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if !metadata.file_type().is_file() {
            return Err(LocalSavedPromptRepositoryError::NotManaged {
                path: path.to_path_buf(),
            });
        }
        let text =
            fs::read_to_string(path).map_err(|source| LocalSavedPromptRepositoryError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let mut documents = serde_yaml::Deserializer::from_str(&text);
        let Some(document) = documents.next() else {
            return Err(LocalSavedPromptRepositoryError::Parse {
                path: path.to_path_buf(),
                source: serde_yaml::from_str::<Workflow>("").unwrap_err(),
            });
        };
        let workflow = Workflow::deserialize(document).map_err(|source| {
            LocalSavedPromptRepositoryError::Parse {
                path: path.to_path_buf(),
                source,
            }
        })?;
        if documents.next().is_some() {
            return Err(LocalSavedPromptRepositoryError::MultipleDocuments {
                path: path.to_path_buf(),
            });
        }
        ensure_agent_mode(&workflow)?;
        Ok(LocalSavedPrompt {
            id,
            workflow,
            path: path.to_path_buf(),
        })
    }

    fn write_atomically(
        &self,
        path: &Path,
        workflow: &Workflow,
        replace_existing: bool,
    ) -> Result<(), LocalSavedPromptRepositoryError> {
        let serialized = serde_yaml::to_string(workflow)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let temp_path = parent.join(format!(
            ".{}.tmp-{}",
            path.file_name().unwrap().to_string_lossy(),
            Uuid::new_v4()
        ));
        let write_result = (|| -> Result<(), LocalSavedPromptRepositoryError> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .map_err(|source| LocalSavedPromptRepositoryError::Io {
                    path: temp_path.clone(),
                    source,
                })?;
            file.write_all(serialized.as_bytes())
                .and_then(|_| file.flush())
                .and_then(|_| file.sync_all())
                .map_err(|source| LocalSavedPromptRepositoryError::Io {
                    path: temp_path.clone(),
                    source,
                })?;
            drop(file);

            if !replace_existing {
                match fs::symlink_metadata(path) {
                    Ok(_) => {
                        return Err(LocalSavedPromptRepositoryError::Io {
                            path: path.to_path_buf(),
                            source: io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "saved prompt exists",
                            ),
                        });
                    }
                    Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(LocalSavedPromptRepositoryError::Io {
                            path: path.to_path_buf(),
                            source,
                        });
                    }
                }
            }
            fs::rename(&temp_path, path).map_err(|source| LocalSavedPromptRepositoryError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            // Directory fsync is supported on Unix and is best-effort on
            // platforms where opening a directory as a File is unavailable.
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }
}

fn ensure_agent_mode(workflow: &Workflow) -> Result<(), LocalSavedPromptRepositoryError> {
    if workflow.is_agent_mode_workflow() {
        Ok(())
    } else {
        Err(LocalSavedPromptRepositoryError::NotAgentMode)
    }
}

fn is_yaml_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("yaml" | "yml")
    )
}

/// Returns whether a path is one of the temporary files used by the atomic
/// saved-prompt writer. These files must remain invisible to the workflow
/// loader and watcher; only the final UUID-named YAML file is meaningful.
pub fn is_atomic_temp_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    file_name.starts_with(ATOMIC_TEMP_PREFIX)
        && file_name.contains(ATOMIC_TEMP_MARKER)
        && !is_yaml_path(path)
}

#[cfg(test)]
#[path = "local_saved_prompts_tests.rs"]
mod tests;
