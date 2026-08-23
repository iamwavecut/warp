//! File-backed local named-agent bundles.
//!
//! A named agent is intentionally just a local YAML document. The filename is
//! the UUID identity, while the user-facing name remains editable data. This
//! module owns parsing, validation, redaction, and the immutable merge used by
//! local entry points.

#![cfg(not(target_family = "wasm"))]

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use warp_cli::agent::{
    AgentCommand, CreateNamedAgentArgs, DeleteNamedAgentArgs, Harness, NamedAgentFieldsArgs,
    NamedAgentSelectorArgs, RunAgentArgs, UpdateNamedAgentArgs,
};
use warp_cli::skill::SkillSpec;
use warpui::{AppContext, platform::TerminationMode};

use crate::ai::agent_sdk::config_file::{
    AgentConfigSnapshotFile, mcp_specs_from_mcp_servers, merge_mcp_servers,
};
use crate::ai::ambient_agents::task::{AgentConfigSnapshot, HarnessConfig};

pub const LOCAL_NAMED_AGENTS_DIR: &str = "agents";
const YAML_EXTENSION: &str = "yaml";
const MAX_BUNDLE_BYTES: u64 = 256 * 1024;
const MAX_NAME_CHARS: usize = 256;

/// References to credentials. Values are names only; this type never stores
/// or resolves the referenced secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBundleSecretRefs {
    /// Logical secret name to environment-variable name.
    #[serde(default)]
    pub env_vars: BTreeMap<String, String>,
    /// Existing keychain/secure-storage entry names.
    #[serde(default)]
    pub keychain_entries: Vec<String>,
}

/// The strict, persisted schema for one local named agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedAgentBundle {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_prompt: Option<String>,
    /// Must be a concrete `custom/<provider>/<model>` or local router ID.
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Ordered local skill names or path references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Unwrapped MCP server map, in the same shape as AgentConfigSnapshotFile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub harness: Harness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_use_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_refs: Option<AgentBundleSecretRefs>,
}

impl NamedAgentBundle {
    pub fn validate(&self) -> Result<(), NamedAgentError> {
        validate_name(&self.name)?;
        validate_model_id(&self.model_id)?;
        if self.harness == Harness::Unknown {
            return Err(NamedAgentError::InvalidBundle {
                reason: "harness must be a known local harness".to_owned(),
            });
        }
        if let Some(profile_id) = &self.profile_id
            && profile_id.trim().is_empty()
        {
            return Err(NamedAgentError::InvalidBundle {
                reason: "profile_id must not be empty".to_owned(),
            });
        }
        for (index, skill) in self.skills.iter().enumerate() {
            validate_skill_reference(skill).map_err(|reason| NamedAgentError::InvalidBundle {
                reason: format!("skills[{index}] {reason}"),
            })?;
        }
        if let Some(secrets) = &self.secret_refs {
            validate_secret_refs(secrets)?;
        }
        validate_prompt(self.description.as_deref(), "description")?;
        validate_prompt(self.base_prompt.as_deref(), "base_prompt")?;
        if let Some(mcp_servers) = &self.mcp_servers {
            let json_servers = btree_to_json_map(mcp_servers);
            validate_named_mcp_servers(&json_servers)?;
        }
        Ok(())
    }

    /// Convert the bundle into the shared runtime snapshot without resolving
    /// any provider, profile, skill, MCP process, or secret.
    pub fn to_snapshot(&self) -> AgentConfigSnapshot {
        AgentConfigSnapshot {
            name: Some(self.name.clone()),
            environment_id: None,
            model_id: Some(self.model_id.clone()),
            base_prompt: self.base_prompt.clone(),
            mcp_servers: self.mcp_servers.as_ref().map(btree_to_json_map),
            profile_id: self.profile_id.clone(),
            worker_host: None,
            skill_spec: self.skills.first().cloned(),
            computer_use_enabled: self.computer_use_enabled,
            harness: Some(HarnessConfig::from_harness_type(self.harness)),
            harness_auth_secrets: None,
        }
    }
}

fn validate_name(name: &str) -> Result<(), NamedAgentError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NamedAgentError::InvalidBundle {
            reason: "name must not be empty".to_owned(),
        });
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(NamedAgentError::InvalidBundle {
            reason: format!("name exceeds {MAX_NAME_CHARS} characters"),
        });
    }
    Ok(())
}

fn validate_model_id(model_id: &str) -> Result<(), NamedAgentError> {
    let valid_custom = model_id
        .strip_prefix("custom/")
        .is_some_and(|rest| rest.split('/').count() == 2 && rest.split('/').all(non_empty_segment));
    let valid_router = model_id
        .strip_prefix("custom-router:local:")
        .is_some_and(non_empty_segment);
    if !valid_custom && !valid_router {
        return Err(NamedAgentError::InvalidBundle {
            reason: "model_id must be a concrete custom/<provider>/<model> or local router id"
                .to_owned(),
        });
    }
    Ok(())
}

fn validate_skill_reference(value: &str) -> Result<SkillSpec, String> {
    let spec = value
        .parse::<SkillSpec>()
        .map_err(|error| format!("is invalid: {error}"))?;
    let identifier = Path::new(&spec.skill_identifier);
    if identifier.is_absolute()
        || identifier
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err("must stay within local skill roots".to_owned());
    }
    Ok(spec)
}

fn validate_prompt(prompt: Option<&str>, field: &str) -> Result<(), NamedAgentError> {
    let Some(prompt) = prompt else {
        return Ok(());
    };
    if literal_secret_like(prompt) {
        return Err(NamedAgentError::SecretValueRejected {
            field: field.to_owned(),
        });
    }
    Ok(())
}

fn literal_secret_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let bearer_token = lower
        .split_whitespace()
        .position(|part| part == "bearer")
        .and_then(|index| lower.split_whitespace().nth(index + 1))
        .is_some_and(|token| token.len() >= 16);
    bearer_token
        || lower.split_whitespace().any(|part| {
            (part.starts_with("sk-") && part.len() >= 20)
                || (part.starts_with("ghp_") && part.len() >= 20)
                || (part.starts_with("xoxb-") && part.len() >= 20)
        })
        || ["api_key=", "apikey=", "password=", "token="]
            .iter()
            .any(|key| {
                lower.find(key).is_some_and(|offset| {
                    let value = lower[offset + key.len()..]
                        .trim_start_matches(|character: char| {
                            character == '"' || character == '\'' || character == '`'
                        })
                        .split_whitespace()
                        .next()
                        .unwrap_or_default();
                    value
                        .trim_matches(|character: char| {
                            character == '"' || character == '\'' || character == '`'
                        })
                        .len()
                        >= 12
                })
            })
}

fn non_empty_segment(segment: &str) -> bool {
    !segment.is_empty() && !segment.chars().any(char::is_whitespace)
}

fn validate_secret_refs(secrets: &AgentBundleSecretRefs) -> Result<(), NamedAgentError> {
    for (logical_name, env_var) in &secrets.env_vars {
        if logical_name.trim().is_empty()
            || logical_name.chars().any(char::is_control)
            || !is_env_var_name(env_var)
        {
            return Err(NamedAgentError::SecretValueRejected {
                field: format!("secret_refs.env_vars.{logical_name}"),
            });
        }
    }
    for (index, entry) in secrets.keychain_entries.iter().enumerate() {
        if entry.trim().is_empty() || entry.chars().any(char::is_control) {
            return Err(NamedAgentError::SecretValueRejected {
                field: format!("secret_refs.keychain_entries[{index}]"),
            });
        }
    }
    Ok(())
}

fn is_env_var_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

fn reject_secret_values(value: &Value, path: &str) -> Result<(), NamedAgentError> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if key.eq_ignore_ascii_case("env") || key.eq_ignore_ascii_case("headers") {
                    let Some(env_map) = child.as_object() else {
                        continue;
                    };
                    for (env_key, env_value) in env_map {
                        if is_secret_key(env_key)
                            || (key.eq_ignore_ascii_case("headers")
                                && env_key.eq_ignore_ascii_case("authorization"))
                        {
                            let Some(raw) = env_value.as_str() else {
                                return Err(NamedAgentError::SecretValueRejected {
                                    field: format!("{child_path}.{env_key}"),
                                });
                            };
                            if !contains_env_ref(raw) {
                                return Err(NamedAgentError::SecretValueRejected {
                                    field: format!("{child_path}.{env_key}"),
                                });
                            }
                        }
                    }
                    continue;
                }
                if is_secret_key(key) {
                    let allowed_reference = child.as_str().is_some_and(contains_env_ref)
                        || (key.ends_with("_env_var")
                            && child.as_str().is_some_and(is_env_var_name));
                    if !allowed_reference {
                        return Err(NamedAgentError::SecretValueRejected { field: child_path });
                    }
                }
                reject_secret_values(child, &child_path)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                reject_secret_values(child, &format!("{path}[{index}]"))?;
            }
        }
        Value::String(string) => {
            if literal_secret_like(string) {
                return Err(NamedAgentError::SecretValueRejected {
                    field: path.to_owned(),
                });
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn parse_env_ref(value: &str) -> Option<&str> {
    value
        .strip_prefix("${")
        .and_then(|name| name.strip_suffix('}'))
        .or_else(|| value.strip_prefix('$'))
        .filter(|name| is_env_var_name(name))
}

fn contains_env_ref(value: &str) -> bool {
    if parse_env_ref(value).is_some() {
        return true;
    }

    for (offset, _) in value.match_indices('$') {
        let remainder = &value[offset + 1..];
        if let Some(name) = remainder.strip_prefix('{') {
            if let Some(end) = name.find('}')
                && is_env_var_name(&name[..end])
            {
                return true;
            }
        } else {
            let name = remainder
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect::<String>();
            if is_env_var_name(&name) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn validate_named_mcp_servers(
    servers: &Map<String, Value>,
) -> Result<(), NamedAgentError> {
    crate::ai::agent_sdk::mcp_config::validate_mcp_servers(servers).map_err(|error| {
        NamedAgentError::InvalidBundle {
            reason: format!("invalid mcp_servers: {error}"),
        }
    })?;
    for (name, server) in servers {
        if server
            .as_object()
            .is_some_and(|object| object.contains_key("warp_id"))
        {
            return Err(NamedAgentError::InvalidBundle {
                reason: format!("mcp_servers.{name}.warp_id is a managed MCP reference"),
            });
        }
    }
    reject_secret_values(&Value::Object(servers.clone()), "mcp_servers")
}

/// Validate one-shot config before any named-agent skill or MCP work starts.
/// Named bundles never inherit hosted workers, managed harness credentials, or
/// cloud MCP references from an override file.
pub fn validate_named_config_file(
    source: &crate::ai::agent_sdk::config_file::AgentConfigSnapshotFile,
) -> Result<(), NamedAgentError> {
    if source.host.is_some() {
        return Err(NamedAgentError::InvalidBundle {
            reason: "one-shot host is not available for local named agents".to_owned(),
        });
    }
    if source.harness_auth_secrets.is_some() {
        return Err(NamedAgentError::InvalidBundle {
            reason: "managed harness authentication is not available for local named agents"
                .to_owned(),
        });
    }
    if let Some(model_id) = &source.model_id {
        validate_model_id(model_id)?;
    }
    if let Some(profile_id) = &source.profile_id
        && profile_id.trim().is_empty()
    {
        return Err(NamedAgentError::InvalidBundle {
            reason: "profile_id must not be empty".to_owned(),
        });
    }
    validate_prompt(source.base_prompt.as_deref(), "one_shot.base_prompt")?;
    if let Some(skill_spec) = &source.skill_spec {
        validate_skill_reference(skill_spec).map_err(|reason| NamedAgentError::InvalidBundle {
            reason: format!("one_shot.skill_spec {reason}"),
        })?;
    }
    if let Some(harness) = &source.harness
        && harness.harness_type == Harness::Unknown
    {
        return Err(NamedAgentError::InvalidBundle {
            reason: "one-shot harness must be a known local harness".to_owned(),
        });
    }
    if let Some(mcp_servers) = &source.mcp_servers {
        validate_named_mcp_servers(mcp_servers)?;
    }
    Ok(())
}

/// Validate fields that can be supplied by the named-agent CLI/UI path. This
/// is intentionally separate from the general CLI path, which still supports
/// legacy hosted-only config files for non-named runs.
pub fn validate_named_run_args(args: &RunAgentArgs) -> Result<(), NamedAgentError> {
    if args.sandboxed {
        return Err(NamedAgentError::InvalidBundle {
            reason: "sandboxed execution is not available for local named agents".to_owned(),
        });
    }
    if let Some(model_id) = &args.model.model {
        validate_model_id(model_id)?;
    }
    if let Some(profile_id) = &args.profile
        && profile_id.trim().is_empty()
    {
        return Err(NamedAgentError::InvalidBundle {
            reason: "profile_id must not be empty".to_owned(),
        });
    }
    if let Some(prompt) = args.prompt_arg.prompt.as_deref() {
        validate_prompt(Some(prompt), "cli.prompt")?;
    }
    if let Some(skill) = &args.skill {
        validate_skill_reference(&skill.to_string()).map_err(|reason| {
            NamedAgentError::InvalidBundle {
                reason: format!("cli.skill {reason}"),
            }
        })?;
    }
    Ok(())
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    [
        "secret",
        "token",
        "password",
        "credential",
        "api_key",
        "apikey",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn btree_to_json_map(value: &BTreeMap<String, Value>) -> Map<String, Value> {
    value
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// A loaded bundle and its content revision.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedAgentRecord {
    id: Uuid,
    bundle: NamedAgentBundle,
    path: PathBuf,
    revision: String,
}

impl NamedAgentRecord {
    pub fn from_parts(id: Uuid, bundle: NamedAgentBundle) -> Self {
        Self {
            id,
            bundle,
            path: PathBuf::new(),
            revision: String::new(),
        }
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn bundle(&self) -> &NamedAgentBundle {
        &self.bundle
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

#[derive(Clone, Debug, Default)]
pub struct NamedAgentList {
    pub agents: Vec<NamedAgentRecord>,
    pub errors: Vec<NamedAgentFileError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedAgentFileError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for NamedAgentFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NamedAgentError {
    #[error("invalid named agent selector '{selector}'")]
    InvalidSelector { selector: String },
    #[error("named agent bundle is invalid: {reason}")]
    InvalidBundle { reason: String },
    #[error("secret value rejected at {field}; use an environment-variable or keychain reference")]
    SecretValueRejected { field: String },
    #[error("named agent {id} already exists")]
    AlreadyExists { id: Uuid },
    #[error("named agent {id} does not exist")]
    NotFound { id: Uuid },
    #[error("named agent name '{name}' is ambiguous")]
    AmbiguousName { name: String },
    #[error("named agent {id} changed while it was being edited")]
    Conflict {
        id: Uuid,
        expected: String,
        actual: Option<String>,
    },
    #[error("named agent file {path} is not a regular managed file")]
    NotManaged { path: PathBuf },
    #[error("named agent file {path} contains multiple YAML documents")]
    MultipleDocuments { path: PathBuf },
    #[error("could not parse named agent file {path} at {location}")]
    Parse { path: PathBuf, location: String },
    #[error("could not access named agent file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not serialize named agent: {0}")]
    Serialize(#[from] serde_yaml::Error),
}

/// File-backed repository. It accepts UUIDs, not arbitrary filenames, so
/// selectors cannot escape the managed directory.
#[derive(Clone, Debug)]
pub struct LocalNamedAgentRepository {
    directory: PathBuf,
}

impl LocalNamedAgentRepository {
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self {
            directory: directory.as_ref().to_path_buf(),
        }
    }
    pub fn for_user() -> Self {
        Self::new(warp_core::paths::data_dir().join(LOCAL_NAMED_AGENTS_DIR))
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn create(&self, bundle: NamedAgentBundle) -> Result<NamedAgentRecord, NamedAgentError> {
        bundle.validate()?;
        let id = Uuid::new_v4();
        let path = self.path_for_id(id)?;
        write_new_atomic(&path, &serialize_bundle(&bundle)?)?;
        self.read(id)
    }

    pub fn get(&self, id: Uuid) -> Result<NamedAgentRecord, NamedAgentError> {
        let path = self.path_for_id(id)?;
        self.read_path(id, &path)
    }
    pub fn read(&self, id: Uuid) -> Result<NamedAgentRecord, NamedAgentError> {
        self.get(id)
    }

    pub fn resolve(&self, selector: &str) -> Result<NamedAgentRecord, NamedAgentError> {
        if let Ok(id) = Uuid::parse_str(selector) {
            return self.get(id);
        }
        let matches = self
            .list_with_errors()?
            .agents
            .into_iter()
            .filter(|record| record.bundle.name == selector)
            .collect::<Vec<_>>();
        match matches.len() {
            0 => Err(NamedAgentError::InvalidSelector {
                selector: selector.to_owned(),
            }),
            1 => Ok(matches.into_iter().next().expect("one match")),
            _ => Err(NamedAgentError::AmbiguousName {
                name: selector.to_owned(),
            }),
        }
    }

    pub fn update(
        &self,
        id: Uuid,
        expected_revision: &str,
        bundle: NamedAgentBundle,
    ) -> Result<NamedAgentRecord, NamedAgentError> {
        bundle.validate()?;
        let path = self.path_for_id(id)?;
        let lock = self.acquire_lock(id)?;
        let current = self.read_path(id, &path)?;
        if current.revision != expected_revision {
            drop(lock);
            return Err(NamedAgentError::Conflict {
                id,
                expected: expected_revision.to_owned(),
                actual: Some(current.revision),
            });
        }
        write_replace_atomic(&path, &serialize_bundle(&bundle)?)?;
        drop(lock);
        self.read(id)
    }

    pub fn delete(
        &self,
        selector: &str,
        expected_revision: Option<&str>,
    ) -> Result<(), NamedAgentError> {
        let id = Uuid::parse_str(selector).map_err(|_| NamedAgentError::InvalidSelector {
            selector: selector.to_owned(),
        })?;
        let path = self.path_for_id(id)?;
        let lock = self.acquire_lock(id)?;
        let current = self.read_path(id, &path)?;
        if let Some(expected) = expected_revision
            && current.revision != expected
        {
            drop(lock);
            return Err(NamedAgentError::Conflict {
                id,
                expected: expected.to_owned(),
                actual: Some(current.revision),
            });
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| NamedAgentError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            drop(lock);
            return Err(NamedAgentError::NotManaged { path });
        }
        fs::remove_file(&path).map_err(|source| NamedAgentError::Io {
            path: path.clone(),
            source,
        })?;
        drop(lock);
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<NamedAgentRecord>, NamedAgentError> {
        let list = self.list_with_errors()?;
        if let Some(error) = list.errors.into_iter().next() {
            return Err(NamedAgentError::Parse {
                path: error.path,
                location: error.message,
            });
        }
        Ok(list.agents)
    }

    pub fn list_with_errors(&self) -> Result<NamedAgentList, NamedAgentError> {
        self.ensure_directory()?;
        let mut result = NamedAgentList::default();
        let entries = fs::read_dir(&self.directory).map_err(|source| NamedAgentError::Io {
            path: self.directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(source) => {
                    result.errors.push(NamedAgentFileError {
                        path: self.directory.clone(),
                        message: source.kind().to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();
            let is_yaml = matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yaml" | "yml")
            );
            if !is_yaml {
                continue;
            }
            let Some(id) = id_from_path(&path) else {
                result.errors.push(NamedAgentFileError {
                    path,
                    message: "filename must be a UUID with a .yaml extension".to_owned(),
                });
                continue;
            };
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    result.errors.push(NamedAgentFileError {
                        path,
                        message: "must be a regular file, not a symlink".to_owned(),
                    });
                }
                Ok(_) => match self.read_path(id, &path) {
                    Ok(record) => result.agents.push(record),
                    Err(error) => result.errors.push(NamedAgentFileError {
                        path,
                        message: safe_file_error_message(&error),
                    }),
                },
                Err(source) => result.errors.push(NamedAgentFileError {
                    path,
                    message: source.kind().to_string(),
                }),
            }
        }
        result
            .agents
            .sort_by_key(|record| (record.bundle.name.to_lowercase(), record.id));
        Ok(result)
    }

    fn read_path(&self, id: Uuid, path: &Path) -> Result<NamedAgentRecord, NamedAgentError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                NamedAgentError::NotFound { id }
            } else {
                NamedAgentError::Io {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(NamedAgentError::NotManaged {
                path: path.to_path_buf(),
            });
        }
        let bytes = fs::read(path).map_err(|source| NamedAgentError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if bytes.len() as u64 > MAX_BUNDLE_BYTES {
            return Err(NamedAgentError::InvalidBundle {
                reason: "bundle exceeds size limit".to_owned(),
            });
        }
        let bundle = parse_bundle(path, &bytes)?;
        Ok(NamedAgentRecord {
            id,
            bundle,
            path: path.to_path_buf(),
            revision: hash_bytes(&bytes),
        })
    }

    fn ensure_directory(&self) -> Result<(), NamedAgentError> {
        if !self.directory.exists() {
            fs::create_dir_all(&self.directory).map_err(|source| NamedAgentError::Io {
                path: self.directory.clone(),
                source,
            })?;
        }
        let metadata =
            fs::symlink_metadata(&self.directory).map_err(|source| NamedAgentError::Io {
                path: self.directory.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(NamedAgentError::NotManaged {
                path: self.directory.clone(),
            });
        }
        Ok(())
    }

    fn path_for_id(&self, id: Uuid) -> Result<PathBuf, NamedAgentError> {
        self.ensure_directory()?;
        Ok(self.directory.join(format!("{id}.{YAML_EXTENSION}")))
    }

    fn acquire_lock(&self, id: Uuid) -> Result<LockFile, NamedAgentError> {
        let path = self.directory.join(format!(".{id}.lock"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    NamedAgentError::Conflict {
                        id,
                        expected: "unlocked".to_owned(),
                        actual: Some("locked".to_owned()),
                    }
                } else {
                    NamedAgentError::Io {
                        path: path.clone(),
                        source,
                    }
                }
            })?;
        Ok(LockFile { path, _file: file })
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

fn serialize_bundle(bundle: &NamedAgentBundle) -> Result<Vec<u8>, NamedAgentError> {
    let bytes = serde_yaml::to_string(bundle)?.into_bytes();
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(NamedAgentError::InvalidBundle {
            reason: "bundle exceeds size limit".to_owned(),
        });
    }
    Ok(bytes)
}

fn parse_bundle(path: &Path, bytes: &[u8]) -> Result<NamedAgentBundle, NamedAgentError> {
    let text = std::str::from_utf8(bytes).map_err(|_| NamedAgentError::Parse {
        path: path.to_path_buf(),
        location: "invalid UTF-8".to_owned(),
    })?;
    let mut documents = serde_yaml::Deserializer::from_str(text);
    let Some(document) = documents.next() else {
        return Err(NamedAgentError::Parse {
            path: path.to_path_buf(),
            location: "empty YAML".to_owned(),
        });
    };
    let bundle =
        NamedAgentBundle::deserialize(document).map_err(|error| NamedAgentError::Parse {
            path: path.to_path_buf(),
            location: yaml_error_location(&error),
        })?;
    if documents.next().is_some() {
        return Err(NamedAgentError::MultipleDocuments {
            path: path.to_path_buf(),
        });
    }
    bundle.validate()?;
    Ok(bundle)
}

fn yaml_error_location(error: &serde_yaml::Error) -> String {
    // Keep the parser's actionable reason (for example, an unknown field),
    // but only retain its first line so a malformed scalar can never be
    // echoed wholesale into list diagnostics.
    let reason = error
        .to_string()
        .lines()
        .next()
        .unwrap_or("invalid YAML")
        .trim()
        .to_owned();
    error
        .location()
        .map(|location| {
            format!(
                "{reason} (line {}, column {})",
                location.line(),
                location.column()
            )
        })
        .unwrap_or(reason)
}

fn safe_file_error_message(error: &NamedAgentError) -> String {
    match error {
        NamedAgentError::Parse { location, .. } => format!("invalid YAML at {location}"),
        NamedAgentError::SecretValueRejected { field } => {
            format!("secret value rejected at {field}")
        }
        NamedAgentError::InvalidBundle { reason } => reason.clone(),
        NamedAgentError::MultipleDocuments { .. } => "contains multiple YAML documents".to_owned(),
        other => other.to_string(),
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn id_from_path(path: &Path) -> Option<Uuid> {
    if !matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yaml" | "yml")
    ) {
        return None;
    }
    Uuid::parse_str(path.file_stem()?.to_str()?).ok()
}

fn write_new_atomic(path: &Path, bytes: &[u8]) -> Result<(), NamedAgentError> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let temp_path = path.with_file_name(format!(".{file_name}.tmp-{}", Uuid::new_v4()));
    write_temp(&temp_path, bytes)?;
    match fs::hard_link(&temp_path, path) {
        Ok(()) => {
            let _ = fs::remove_file(&temp_path);
            sync_parent(path);
            Ok(())
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temp_path);
            Err(NamedAgentError::AlreadyExists {
                id: Uuid::parse_str(
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default(),
                )
                .unwrap_or_else(|_| Uuid::nil()),
            })
        }
        Err(source) => {
            let _ = fs::remove_file(&temp_path);
            Err(NamedAgentError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    }
}

fn write_replace_atomic(path: &Path, bytes: &[u8]) -> Result<(), NamedAgentError> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let temp_path = path.with_file_name(format!(".{file_name}.tmp-{}", Uuid::new_v4()));
    write_temp(&temp_path, bytes)?;
    if let Err(source) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(NamedAgentError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    sync_parent(path);
    Ok(())
}

fn write_temp(path: &Path, bytes: &[u8]) -> Result<(), NamedAgentError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| NamedAgentError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|source| NamedAgentError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(file) = File::open(parent)
    {
        let _ = file.sync_all();
    }
}

/// One-shot and command-line values used when resolving a stored bundle.
#[derive(Clone, Debug, Default)]
pub struct NamedAgentRunOverrides {
    pub one_shot: Option<AgentConfigSnapshotFile>,
    pub cli: AgentConfigSnapshot,
    pub bundle_skill_instructions: Option<String>,
    pub invoked_skill_instructions: Option<String>,
}

/// Merge a named bundle with one-shot and CLI overrides. The input bundle is
/// borrowed and never mutated. Invoked skill instructions are applied last.
pub fn merge_named_agent_config(
    bundle: &NamedAgentBundle,
    overrides: &NamedAgentRunOverrides,
) -> Result<AgentConfigSnapshot, NamedAgentError> {
    bundle.validate()?;
    if let Some(one_shot) = &overrides.one_shot {
        validate_named_config_file(one_shot)?;
    }
    validate_named_snapshot(&overrides.cli, "cli", false)?;
    let mut merged = bundle.to_snapshot();
    if let Some(instructions) = &overrides.bundle_skill_instructions {
        merged.base_prompt = Some(match merged.base_prompt.take() {
            Some(base_prompt) if !base_prompt.trim().is_empty() => {
                format!("{base_prompt}\n\n{instructions}")
            }
            _ => instructions.clone(),
        });
    }
    if let Some(one_shot) = &overrides.one_shot {
        apply_file_snapshot(&mut merged, one_shot);
    }
    apply_snapshot(&mut merged, &overrides.cli);
    if let Some(instructions) = &overrides.invoked_skill_instructions {
        merged.base_prompt = Some(instructions.clone());
    }
    validate_named_snapshot(&merged, "effective", true)?;
    Ok(merged)
}

fn validate_named_snapshot(
    snapshot: &AgentConfigSnapshot,
    source: &str,
    require_model: bool,
) -> Result<(), NamedAgentError> {
    if snapshot.environment_id.is_some() {
        return Err(NamedAgentError::InvalidBundle {
            reason: format!("{source}.environment_id is not available for local named agents"),
        });
    }
    if snapshot.worker_host.is_some() {
        return Err(NamedAgentError::InvalidBundle {
            reason: format!("{source}.host is not available for local named agents"),
        });
    }
    if snapshot.harness_auth_secrets.is_some() {
        return Err(NamedAgentError::InvalidBundle {
            reason: format!(
                "{source}.harness_auth_secrets is not available for local named agents"
            ),
        });
    }
    if let Some(name) = &snapshot.name {
        validate_name(name)?;
    }
    match snapshot.model_id.as_deref() {
        Some(model_id) => validate_model_id(model_id)?,
        None if require_model => {
            return Err(NamedAgentError::InvalidBundle {
                reason: "effective model_id must be concrete".to_owned(),
            });
        }
        None => {}
    }
    if let Some(profile_id) = &snapshot.profile_id
        && profile_id.trim().is_empty()
    {
        return Err(NamedAgentError::InvalidBundle {
            reason: format!("{source}.profile_id must not be empty"),
        });
    }
    validate_prompt(
        snapshot.base_prompt.as_deref(),
        &format!("{source}.base_prompt"),
    )?;
    if let Some(skill_spec) = &snapshot.skill_spec {
        validate_skill_reference(skill_spec).map_err(|reason| NamedAgentError::InvalidBundle {
            reason: format!("{source}.skill_spec {reason}"),
        })?;
    }
    if let Some(harness) = &snapshot.harness
        && harness.harness_type == Harness::Unknown
    {
        return Err(NamedAgentError::InvalidBundle {
            reason: format!("{source}.harness must be a known local harness"),
        });
    }
    if let Some(mcp_servers) = &snapshot.mcp_servers {
        validate_named_mcp_servers(mcp_servers)?;
    }
    Ok(())
}

fn apply_file_snapshot(target: &mut AgentConfigSnapshot, source: &AgentConfigSnapshotFile) {
    if source.name.is_some() {
        target.name = source.name.clone();
    }
    if source.model_id.is_some() {
        target.model_id = source.model_id.clone();
    }
    if source.base_prompt.is_some() {
        target.base_prompt = source.base_prompt.clone();
    }
    if source.mcp_servers.is_some() {
        target.mcp_servers =
            merge_mcp_servers(target.mcp_servers.take(), source.mcp_servers.clone());
    }
    if source.profile_id.is_some() {
        target.profile_id = source.profile_id.clone();
    }
    if source.host.is_some() {
        target.worker_host = source.host.clone();
    }
    if source.skill_spec.is_some() {
        target.skill_spec = source.skill_spec.clone();
    }
    if source.computer_use_enabled.is_some() {
        target.computer_use_enabled = source.computer_use_enabled;
    }
    if source.harness.is_some() {
        target.harness = source.harness.clone();
    }
    if source.harness_auth_secrets.is_some() {
        target.harness_auth_secrets = source.harness_auth_secrets.clone();
    }
}

fn apply_snapshot(target: &mut AgentConfigSnapshot, source: &AgentConfigSnapshot) {
    if source.name.is_some() {
        target.name = source.name.clone();
    }
    if source.model_id.is_some() {
        target.model_id = source.model_id.clone();
    }
    if source.base_prompt.is_some() {
        target.base_prompt = source.base_prompt.clone();
    }
    if source.mcp_servers.is_some() {
        target.mcp_servers =
            merge_mcp_servers(target.mcp_servers.take(), source.mcp_servers.clone());
    }
    if source.profile_id.is_some() {
        target.profile_id = source.profile_id.clone();
    }
    if source.worker_host.is_some() {
        target.worker_host = source.worker_host.clone();
    }
    if source.skill_spec.is_some() {
        target.skill_spec = source.skill_spec.clone();
    }
    if source.computer_use_enabled.is_some() {
        target.computer_use_enabled = source.computer_use_enabled;
    }
    if source.harness.is_some() {
        target.harness = source.harness.clone();
    }
    if source.harness_auth_secrets.is_some() {
        target.harness_auth_secrets = source.harness_auth_secrets.clone();
    }
}

/// Human-safe default list output. Prompts and secret reference names are
/// deliberately absent; details must be requested explicitly by ID.
pub fn format_named_agent_list(records: &[NamedAgentRecord]) -> String {
    let mut output = String::new();
    for record in records {
        output.push_str("local named agent\t");
        output.push_str(&record.id.to_string());
        output.push('\t');
        output.push_str(&record.bundle.name);
        output.push('\n');
    }
    output
}

/// Build runtime MCP specs only after a bundle has passed all local checks.
pub fn runtime_mcp_specs(
    bundle: &NamedAgentBundle,
) -> Result<Vec<warp_cli::mcp::MCPSpec>, NamedAgentError> {
    bundle.validate()?;
    match bundle.mcp_servers.as_ref() {
        Some(servers) => mcp_specs_from_mcp_servers(&btree_to_json_map(servers)).map_err(|error| {
            NamedAgentError::InvalidBundle {
                reason: error.to_string(),
            }
        }),
        None => Ok(Vec::new()),
    }
}

/// Handle local named-agent CRUD commands. These operations only touch the
/// local repository and terminate the headless CLI process when complete.
pub fn run_named_agent_crud(ctx: &mut AppContext, command: AgentCommand) -> anyhow::Result<()> {
    match command {
        AgentCommand::Create(args) => create_from_cli(ctx, args),
        AgentCommand::Show(args) => show_from_cli(ctx, args),
        AgentCommand::Update(args) => update_from_cli(ctx, args),
        AgentCommand::Delete(args) => delete_from_cli(ctx, args),
        _ => Err(anyhow::anyhow!("not a named-agent CRUD command")),
    }
}

fn create_from_cli(ctx: &mut AppContext, args: CreateNamedAgentArgs) -> anyhow::Result<()> {
    let bundle = bundle_from_fields(&args.fields, None)?;
    let record = LocalNamedAgentRepository::for_user().create(bundle)?;
    println!(
        "created local named agent {} ({})",
        record.bundle.name, record.id
    );
    ctx.terminate_app(TerminationMode::ForceTerminate, None);
    Ok(())
}

fn show_from_cli(ctx: &mut AppContext, args: NamedAgentSelectorArgs) -> anyhow::Result<()> {
    let record = LocalNamedAgentRepository::for_user().resolve(&args.selector)?;
    let mut bundle = record.bundle.clone();
    bundle.secret_refs = None;
    println!("id: {}\nrevision: {}", record.id, record.revision);
    println!("{}", serde_yaml::to_string(&bundle)?);
    ctx.terminate_app(TerminationMode::ForceTerminate, None);
    Ok(())
}

fn update_from_cli(ctx: &mut AppContext, args: UpdateNamedAgentArgs) -> anyhow::Result<()> {
    let repository = LocalNamedAgentRepository::for_user();
    let current = repository.resolve(&args.selector)?;
    let bundle = bundle_from_fields(&args.fields, Some(&current.bundle))?;
    let record = repository.update(current.id, &args.revision, bundle)?;
    println!(
        "updated local named agent {} ({})",
        record.bundle.name, record.id
    );
    ctx.terminate_app(TerminationMode::ForceTerminate, None);
    Ok(())
}

fn delete_from_cli(ctx: &mut AppContext, args: DeleteNamedAgentArgs) -> anyhow::Result<()> {
    if !args.yes {
        return Err(anyhow::anyhow!(
            "deleting a local named agent requires --yes"
        ));
    }
    LocalNamedAgentRepository::for_user().delete(&args.selector, Some(&args.revision))?;
    println!("deleted local named agent {}", args.selector);
    ctx.terminate_app(TerminationMode::ForceTerminate, None);
    Ok(())
}

fn bundle_from_fields(
    fields: &NamedAgentFieldsArgs,
    existing: Option<&NamedAgentBundle>,
) -> anyhow::Result<NamedAgentBundle> {
    let name = fields
        .name
        .clone()
        .or_else(|| existing.map(|bundle| bundle.name.clone()))
        .ok_or_else(|| anyhow::anyhow!("--name is required"))?;
    let model_id = fields
        .model_id
        .clone()
        .or_else(|| existing.map(|bundle| bundle.model_id.clone()))
        .ok_or_else(|| anyhow::anyhow!("--model is required"))?;
    let mcp_servers = if fields.mcp_specs.is_empty() {
        existing.and_then(|bundle| bundle.mcp_servers.clone())
    } else {
        let map =
            crate::ai::agent_sdk::mcp_config::build_mcp_servers_from_specs(&fields.mcp_specs)?
                .map(|map| map.into_iter().collect())
                .or_else(|| Some(BTreeMap::new()));
        map
    };
    let secret_refs = if fields.secret_env.is_empty() && fields.secret_keychain.is_empty() {
        existing.and_then(|bundle| bundle.secret_refs.clone())
    } else {
        let env_vars = fields
            .secret_env
            .iter()
            .map(|entry| {
                entry
                    .split_once('=')
                    .map(|(name, env)| (name.to_owned(), env.to_owned()))
                    .ok_or_else(|| anyhow::anyhow!("--secret-env expects NAME=ENV_VAR"))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        Some(AgentBundleSecretRefs {
            env_vars,
            keychain_entries: fields.secret_keychain.clone(),
        })
    };
    Ok(NamedAgentBundle {
        name,
        description: fields
            .description
            .clone()
            .or_else(|| existing.and_then(|bundle| bundle.description.clone())),
        base_prompt: fields
            .base_prompt
            .clone()
            .or_else(|| existing.and_then(|bundle| bundle.base_prompt.clone())),
        model_id,
        profile_id: fields
            .profile_id
            .clone()
            .or_else(|| existing.and_then(|bundle| bundle.profile_id.clone())),
        skills: if fields.skills.is_empty() {
            existing
                .map(|bundle| bundle.skills.clone())
                .unwrap_or_default()
        } else {
            fields.skills.iter().map(ToString::to_string).collect()
        },
        mcp_servers,
        harness: fields
            .harness
            .or_else(|| existing.map(|bundle| bundle.harness))
            .unwrap_or(Harness::Oz),
        computer_use_enabled: fields
            .computer_use_override()
            .or_else(|| existing.and_then(|bundle| bundle.computer_use_enabled)),
        secret_refs,
    })
}

#[cfg(test)]
#[path = "local_named_agents_tests.rs"]
mod tests;
