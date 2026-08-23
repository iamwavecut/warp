//! Local, file-backed custom model routers.
//!
//! A router is deliberately resolved in this process. Its YAML definition is
//! only a local editing/catalog concern; the direct OpenAI-compatible adapter
//! receives the concrete `custom/<provider>/<model>` id selected here.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

#[cfg(unix)]
use nix::fcntl::{FlockArg, flock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use unicode_normalization_alignments::UnicodeNormalization;
use uuid::Uuid;

use crate::ai::agent::api::direct_openai::{
    EffectiveCustomProviderCapabilities, effective_capabilities_for_config,
};
use crate::ai::llms::{
    DisableReason, LLMCapabilities, LLMContextWindow, LLMId, LLMInfo, LLMProvider, LLMUsageMetadata,
};
use crate::settings::{
    CUSTOM_PROVIDER_MIN_CONTEXT_WINDOW_TOKENS, CustomProviderConfig, custom_provider_name_is_unique,
};

/// Prefix shared by the picker identity and persisted local selection.
pub const CUSTOM_ROUTER_PREFIX: &str = "custom-router:";
/// Prefix for routers backed by a local YAML file.
pub const LOCAL_CUSTOM_ROUTER_PREFIX: &str = "custom-router:local:";

const MAX_ROUTER_PROMPT_CHARS: usize = 64 * 1024;
const MAX_ROUTER_YAML_BYTES: usize = 256 * 1024;
const MAX_ROUTER_RULES: usize = 128;
const MAX_ROUTER_TARGETS: usize = 132;
const MAX_ROUTER_ID_CHARS: usize = 256;
const MAX_COMPLEXITY_CONTEXT_CHARS: usize = 1_000_000;

/// Routing strategy authored in a router YAML file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomModelRouting {
    Complexity(ComplexityRouting),
    Prompt(PromptRouting),
}

/// A deterministic complexity router. Missing optional buckets use `default`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplexityRouting {
    pub default: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard: Option<String>,
}

/// An ordered, token-based prompt router. No classifier or network request is
/// involved: the first rule with a meaningful token match wins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRouting {
    pub default_model: String,
    #[serde(default)]
    pub rules: Vec<PromptRule>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRule {
    pub description: String,
    pub model: String,
}

/// A single local custom model router.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomModelRouter {
    pub info: LLMInfo,
    pub routing: CustomModelRouting,
    pub source_path: Option<PathBuf>,
}

impl CustomModelRouter {
    pub fn new_local(
        name: String,
        routing: CustomModelRouting,
        source_path: Option<&Path>,
    ) -> Self {
        let id = local_id_from_path(source_path, &name);
        let routing_description = match &routing {
            CustomModelRouting::Complexity(_) => "Routes by task complexity",
            CustomModelRouting::Prompt(_) => "Routes by prompt content",
        };
        let description = source_path
            .map(|path| {
                format!(
                    "{routing_description} · {}",
                    warp_core::paths::home_relative_path(path)
                )
            })
            .unwrap_or_else(|| routing_description.to_owned());
        Self {
            info: LLMInfo {
                display_name: name.clone(),
                base_model_name: name,
                id: format!("{LOCAL_CUSTOM_ROUTER_PREFIX}{id}").into(),
                reasoning_level: None,
                usage_metadata: LLMUsageMetadata {
                    request_multiplier: 1,
                    credit_multiplier: None,
                },
                description: Some(description),
                disable_reason: None,
                // The parser does not know the target catalog yet. The
                // catalog builder replaces this with the conservative
                // intersection of reachable target capabilities.
                vision_supported: false,
                spec: None,
                provider: LLMProvider::Unknown,
                host_configs: HashMap::new(),
                context_window: LLMContextWindow::default(),
                capabilities: None,
            },
            routing,
            source_path: source_path.map(Path::to_path_buf),
        }
    }

    pub fn config_key(&self) -> String {
        self.info.id.as_str().to_owned()
    }

    pub fn llm_id(&self) -> LLMId {
        self.info.id.clone()
    }

    /// Return a copy whose display name changed without changing the file/id.
    pub fn with_display_name(&self, display_name: String) -> Self {
        let mut clone = self.clone();
        clone.info.display_name = display_name.clone();
        clone.info.base_model_name = display_name;
        clone
    }

    /// Return a copy tied to a managed path. The filename, rather than the
    /// display name, is the durable local identity.
    pub fn with_source_path(&self, source_path: &Path) -> Self {
        let mut clone = self.clone();
        let id = local_id_from_path(Some(source_path), &clone.info.display_name);
        clone.info.id = format!("{LOCAL_CUSTOM_ROUTER_PREFIX}{id}").into();
        clone.info.description = Some(format!(
            "{} · {}",
            match &clone.routing {
                CustomModelRouting::Complexity(_) => "Routes by task complexity",
                CustomModelRouting::Prompt(_) => "Routes by prompt content",
            },
            warp_core::paths::home_relative_path(source_path)
        ));
        clone.source_path = Some(source_path.to_path_buf());
        clone
    }

    pub fn all_targets(&self) -> Vec<&str> {
        match &self.routing {
            CustomModelRouting::Complexity(routing) => std::iter::once(routing.default.as_str())
                .chain(routing.easy.as_deref())
                .chain(routing.medium.as_deref())
                .chain(routing.hard.as_deref())
                .collect(),
            CustomModelRouting::Prompt(routing) => std::iter::once(routing.default_model.as_str())
                .chain(routing.rules.iter().map(|rule| rule.model.as_str()))
                .collect(),
        }
    }

    /// Validate YAML-level invariants which do not require provider settings.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.info.display_name.trim();
        if name.chars().count() > MAX_ROUTER_ID_CHARS {
            return Err(format!(
                "custom model router name exceeds {MAX_ROUTER_ID_CHARS} characters"
            ));
        }
        if name.is_empty() {
            return Err("custom model router `name` is empty".to_owned());
        }
        if self.all_targets().len() > MAX_ROUTER_TARGETS {
            return Err(format!("router has more than {MAX_ROUTER_TARGETS} targets"));
        }
        match &self.routing {
            CustomModelRouting::Complexity(routing) => {
                validate_target(&routing.default).map_err(|error| format!("`default`: {error}"))?;
                for (bucket, target) in [
                    ("easy", routing.easy.as_deref()),
                    ("medium", routing.medium.as_deref()),
                    ("hard", routing.hard.as_deref()),
                ] {
                    if let Some(target) = target {
                        validate_target(target)
                            .map_err(|error| format!("complexity bucket `{bucket}`: {error}"))?;
                    }
                }
            }
            CustomModelRouting::Prompt(routing) => {
                if routing.rules.len() > MAX_ROUTER_RULES {
                    return Err(format!(
                        "router has more than {MAX_ROUTER_RULES} prompt rules"
                    ));
                }
                validate_target(&routing.default_model)
                    .map_err(|error| format!("`default`: {error}"))?;
                for (index, rule) in routing.rules.iter().enumerate() {
                    if meaningful_tokens(&rule.description).is_empty() {
                        return Err(format!("prompt rule {index}: `description` is empty"));
                    }
                    validate_target(&rule.model)
                        .map_err(|error| format!("prompt rule {index}: {error}"))?;
                }
            }
        }
        Ok(())
    }

    /// Resolve only the deterministic routing decision. Provider existence,
    /// capabilities, and context limits are checked by [`resolve_router`].
    pub fn resolve(&self, facts: &RouterRequestFacts) -> Result<RouterSelection, String> {
        self.validate()?;
        let (model_id, bucket, rule_index) = match &self.routing {
            CustomModelRouting::Complexity(routing) => {
                let bucket = complexity_bucket(facts);
                let model = match bucket {
                    ComplexityBucket::Easy => routing.easy.as_deref(),
                    ComplexityBucket::Medium => routing.medium.as_deref(),
                    ComplexityBucket::Hard => routing.hard.as_deref(),
                }
                .unwrap_or(&routing.default);
                (model.to_owned(), Some(bucket), None)
            }
            CustomModelRouting::Prompt(routing) => {
                let prompt_tokens = meaningful_tokens(&facts.prompt);
                let selected = routing.rules.iter().enumerate().find(|(_, rule)| {
                    let rule_tokens = meaningful_tokens(&rule.description);
                    !rule_tokens.is_empty()
                        && rule_tokens.iter().any(|token| {
                            prompt_tokens
                                .iter()
                                .any(|prompt_token| prompt_token == token)
                        })
                });
                match selected {
                    Some((index, rule)) => (rule.model.clone(), None, Some(index)),
                    None => (routing.default_model.clone(), None, None),
                }
            }
        };
        Ok(RouterSelection {
            router_id: self.llm_id(),
            model_id,
            complexity_bucket: bucket,
            prompt_rule_index: rule_index,
        })
    }

    pub fn to_yaml_string(&self) -> Result<String, serde_yaml::Error> {
        match &self.routing {
            CustomModelRouting::Complexity(routing) => {
                let routing_yaml =
                    (routing.easy.is_some() || routing.medium.is_some() || routing.hard.is_some())
                        .then(|| {
                            serde_yaml::to_value(YamlOutputComplexityRouting {
                                easy: routing.easy.as_deref(),
                                medium: routing.medium.as_deref(),
                                hard: routing.hard.as_deref(),
                            })
                        })
                        .transpose()?;
                serde_yaml::to_string(&YamlOutputRouter {
                    name: &self.info.display_name,
                    model_type: "complexity",
                    default: &routing.default,
                    routing: routing_yaml,
                })
            }
            CustomModelRouting::Prompt(routing) => {
                let rules = routing
                    .rules
                    .iter()
                    .map(|rule| YamlOutputPromptRule {
                        description: &rule.description,
                        model: &rule.model,
                    })
                    .collect::<Vec<_>>();
                let routing_yaml = (!rules.is_empty())
                    .then(|| serde_yaml::to_value(rules))
                    .transpose()?;
                serde_yaml::to_string(&YamlOutputRouter {
                    name: &self.info.display_name,
                    model_type: "prompt",
                    default: &routing.default_model,
                    routing: routing_yaml,
                })
            }
        }
    }
}

/// A request summary used by deterministic routing. Callers should populate
/// it from facts already available locally; no prompt is sent to a classifier.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouterRequestFacts {
    pub prompt: String,
    pub context_chars: usize,
    pub attachment_count: usize,
    pub requires_code_review: bool,
    pub requires_edit: bool,
    pub requires_tools: bool,
    pub requires_vision: bool,
}

impl RouterRequestFacts {
    pub fn from_prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComplexityBucket {
    Easy,
    Medium,
    Hard,
}

/// Selection details used for local diagnostics. Only these fields should be
/// logged; prompt contents are intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterSelection {
    pub router_id: LLMId,
    pub model_id: String,
    pub complexity_bucket: Option<ComplexityBucket>,
    pub prompt_rule_index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCustomTarget {
    pub provider_name: String,
    pub model_id: String,
    pub base_url: String,
    pub capabilities: EffectiveCustomProviderCapabilities,
    pub context_window_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouterResolutionError {
    InvalidRouter(String),
    InvalidTarget(String),
    AmbiguousProvider(String),
    MissingProvider(String),
    MissingModel(String),
    InvalidProvider {
        provider: String,
        reason: String,
    },
    CapabilityMismatch {
        target: String,
        capability: &'static str,
    },
    ContextWindowTooSmall {
        target: String,
        required_chars: usize,
        limit_tokens: u32,
    },
}

impl std::fmt::Display for RouterResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRouter(error) => write!(f, "invalid custom model router: {error}"),
            Self::InvalidTarget(target) => {
                write!(f, "router target `{target}` is not a concrete custom model")
            }
            Self::AmbiguousProvider(provider) => {
                write!(f, "custom provider name `{provider}` is ambiguous")
            }
            Self::MissingProvider(provider) => {
                write!(f, "custom provider `{provider}` is not configured")
            }
            Self::MissingModel(target) => write!(f, "custom model `{target}` is not configured"),
            Self::InvalidProvider { provider, reason } => {
                write!(f, "custom provider `{provider}` is invalid: {reason}")
            }
            Self::CapabilityMismatch { target, capability } => {
                write!(f, "custom model `{target}` does not provide `{capability}`")
            }
            Self::ContextWindowTooSmall {
                target,
                required_chars,
                limit_tokens,
            } => write!(
                f,
                "custom model `{target}` has too little context for {required_chars} local characters ({limit_tokens} tokens)"
            ),
        }
    }
}

impl std::error::Error for RouterResolutionError {}

/// Resolve one router to a concrete configured custom provider model.
pub fn resolve_router(
    router: &CustomModelRouter,
    facts: &RouterRequestFacts,
    providers: &[CustomProviderConfig],
) -> Result<ResolvedCustomTarget, RouterResolutionError> {
    let selection = router
        .resolve(facts)
        .map_err(RouterResolutionError::InvalidRouter)?;
    resolve_target(&selection.model_id, facts, providers)
}

/// Resolve and retain the diagnostic selection alongside the concrete target.
pub fn resolve_router_selection(
    router: &CustomModelRouter,
    facts: &RouterRequestFacts,
    providers: &[CustomProviderConfig],
) -> Result<(RouterSelection, ResolvedCustomTarget), RouterResolutionError> {
    let selection = router
        .resolve(facts)
        .map_err(RouterResolutionError::InvalidRouter)?;
    let target = resolve_target(&selection.model_id, facts, providers)?;
    log::debug!(
        "local custom router resolved: router_id={} bucket={:?} rule_index={:?} model_id={}",
        selection.router_id,
        selection.complexity_bucket,
        selection.prompt_rule_index,
        selection.model_id
    );
    Ok((selection, target))
}

fn resolve_target(
    target: &str,
    facts: &RouterRequestFacts,
    providers: &[CustomProviderConfig],
) -> Result<ResolvedCustomTarget, RouterResolutionError> {
    let (provider_name, model_id) = parse_concrete_custom_model_id(target)
        .ok_or_else(|| RouterResolutionError::InvalidTarget(target.to_owned()))?;
    if !custom_provider_name_is_unique(provider_name, providers) {
        if providers
            .iter()
            .any(|provider| provider.name == provider_name)
        {
            return Err(RouterResolutionError::AmbiguousProvider(
                provider_name.to_owned(),
            ));
        }
        return Err(RouterResolutionError::MissingProvider(
            provider_name.to_owned(),
        ));
    }
    let provider = providers
        .iter()
        .find(|provider| provider.name == provider_name)
        .ok_or_else(|| RouterResolutionError::MissingProvider(provider_name.to_owned()))?;
    validate_provider(provider)?;
    if !provider.models.iter().any(|model| model == model_id) {
        return Err(RouterResolutionError::MissingModel(target.to_owned()));
    }
    let capabilities = effective_capabilities_for_config(&provider.capabilities);
    let requires_tools = facts.requires_tools || facts.requires_code_review || facts.requires_edit;
    if !capabilities.chat {
        return Err(RouterResolutionError::CapabilityMismatch {
            target: target.to_owned(),
            capability: "chat",
        });
    }
    if requires_tools && !capabilities.tools {
        return Err(RouterResolutionError::CapabilityMismatch {
            target: target.to_owned(),
            capability: "tools",
        });
    }
    if facts.requires_vision && !capabilities.vision {
        return Err(RouterResolutionError::CapabilityMismatch {
            target: target.to_owned(),
            capability: "vision",
        });
    }
    if let Some(limit_tokens) = provider.capabilities.context_window_tokens {
        let limit_chars = (limit_tokens as usize).saturating_mul(3);
        if facts.context_chars.min(MAX_COMPLEXITY_CONTEXT_CHARS) > limit_chars {
            return Err(RouterResolutionError::ContextWindowTooSmall {
                target: target.to_owned(),
                required_chars: facts.context_chars,
                limit_tokens,
            });
        }
    }
    Ok(ResolvedCustomTarget {
        provider_name: provider.name.clone(),
        model_id: model_id.to_owned(),
        base_url: provider.base_url.clone(),
        capabilities,
        context_window_tokens: provider.capabilities.context_window_tokens,
    })
}

fn validate_provider(provider: &CustomProviderConfig) -> Result<(), RouterResolutionError> {
    provider
        .validate()
        .map_err(|error| RouterResolutionError::InvalidProvider {
            provider: provider.name.clone(),
            reason: error.to_string(),
        })?;
    if provider.name.trim().is_empty() {
        return Err(RouterResolutionError::InvalidProvider {
            provider: provider.name.clone(),
            reason: "provider name is empty".to_owned(),
        });
    }
    if provider.base_url.trim().is_empty() {
        return Err(RouterResolutionError::InvalidProvider {
            provider: provider.name.clone(),
            reason: "base URL is empty".to_owned(),
        });
    }
    let parsed_url = url::Url::parse(provider.base_url.trim()).map_err(|error| {
        RouterResolutionError::InvalidProvider {
            provider: provider.name.clone(),
            reason: format!("base URL is invalid: {error}"),
        }
    })?;
    if !matches!(parsed_url.scheme(), "http" | "https") || parsed_url.host().is_none() {
        return Err(RouterResolutionError::InvalidProvider {
            provider: provider.name.clone(),
            reason: "base URL must use http(s) and include a host".to_owned(),
        });
    }
    if provider.models.is_empty() || provider.models.iter().any(|model| model.trim().is_empty()) {
        return Err(RouterResolutionError::InvalidProvider {
            provider: provider.name.clone(),
            reason: "model list contains no concrete model".to_owned(),
        });
    }
    Ok(())
}

fn parse_concrete_custom_model_id(value: &str) -> Option<(&str, &str)> {
    if value.len() > MAX_ROUTER_ID_CHARS {
        return None;
    }
    let remainder = value.strip_prefix("custom/")?;
    let (provider, model) = remainder.split_once('/')?;
    if provider.trim().is_empty()
        || model.trim().is_empty()
        || provider.contains('/')
        || provider.contains(char::is_whitespace)
        || model.contains(char::is_whitespace)
        || model.starts_with("router:")
        || model.starts_with(CUSTOM_ROUTER_PREFIX)
    {
        return None;
    }
    Some((provider, model))
}

fn validate_target(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("target model id is empty".to_owned());
    }
    if is_auto_target(value) {
        return Err(format!("target `{value}` is an auto or nested router"));
    }
    if parse_concrete_custom_model_id(value).is_none() {
        return Err(format!(
            "target `{value}` is not a concrete custom/<provider>/<model> id"
        ));
    }
    Ok(())
}

pub fn is_auto_target(value: &str) -> bool {
    let value = value.trim();
    value == "auto"
        || value.starts_with("auto-")
        || value == "cli-agent-auto"
        || value == "computer-use-agent-auto"
        || value.starts_with(CUSTOM_ROUTER_PREFIX)
}

pub fn is_custom_router_id(value: &str) -> bool {
    value.starts_with(CUSTOM_ROUTER_PREFIX)
}

pub fn is_local_custom_router_id(value: &str) -> bool {
    value.starts_with(LOCAL_CUSTOM_ROUTER_PREFIX)
}

pub fn complexity_bucket(facts: &RouterRequestFacts) -> ComplexityBucket {
    let context_chars = facts.context_chars.min(MAX_COMPLEXITY_CONTEXT_CHARS);
    if facts.requires_code_review
        || facts.requires_edit
        || context_chars >= 96_000
        || facts.attachment_count >= 4
    {
        ComplexityBucket::Hard
    } else if facts.requires_tools || facts.attachment_count > 0 || context_chars >= 12_000 {
        ComplexityBucket::Medium
    } else {
        ComplexityBucket::Easy
    }
}

fn meaningful_tokens(value: &str) -> Vec<String> {
    value
        .chars()
        .take(MAX_ROUTER_PROMPT_CHARS)
        .nfkc()
        .map(|(character, _)| character)
        .flat_map(|character| character.to_lowercase())
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|token| {
            let chars = token.chars().count();
            chars > 1
                && !matches!(
                    *token,
                    "a" | "an" | "and" | "for" | "in" | "of" | "the" | "to" | "with"
                )
        })
        .map(str::to_owned)
        .collect()
}

/// Conservative capability metadata for a router's reachable targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterCatalogCapabilities {
    pub chat: bool,
    pub tools: bool,
    pub vision: bool,
    pub embeddings: bool,
    pub transcription: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouterCatalogEntry {
    pub router: CustomModelRouter,
    pub info: LLMInfo,
    pub capabilities: RouterCatalogCapabilities,
    pub context_window_tokens: Option<u32>,
    pub disable_reason: Option<String>,
}

impl RouterCatalogEntry {
    pub fn is_available(&self) -> bool {
        self.disable_reason.is_none()
    }
}

pub fn router_catalog_entry(
    router: &CustomModelRouter,
    providers: &[CustomProviderConfig],
) -> Result<RouterCatalogEntry, RouterResolutionError> {
    router
        .validate()
        .map_err(RouterResolutionError::InvalidRouter)?;
    let targets = router
        .all_targets()
        .into_iter()
        .map(|target| resolve_target(target, &RouterRequestFacts::default(), providers))
        .collect::<Result<Vec<_>, _>>()?;
    if targets.is_empty() {
        return Err(RouterResolutionError::InvalidRouter(
            "router has no targets".to_owned(),
        ));
    }
    let capabilities = RouterCatalogCapabilities {
        chat: targets.iter().all(|target| target.capabilities.chat),
        tools: targets.iter().all(|target| target.capabilities.tools),
        vision: targets.iter().all(|target| target.capabilities.vision),
        embeddings: targets.iter().all(|target| target.capabilities.embeddings),
        transcription: targets
            .iter()
            .all(|target| target.capabilities.transcription),
    };
    let context_window_tokens = targets
        .iter()
        .filter_map(|target| target.context_window_tokens)
        .min();
    let mut info = router.info.clone();
    info.vision_supported = capabilities.vision;
    info.capabilities = Some(LLMCapabilities {
        chat: capabilities.chat,
        tools: capabilities.tools,
        vision: capabilities.vision,
        embeddings: capabilities.embeddings,
        transcription: capabilities.transcription,
    });
    if let Some(tokens) = context_window_tokens {
        info.context_window = LLMContextWindow {
            is_configurable: false,
            min: CUSTOM_PROVIDER_MIN_CONTEXT_WINDOW_TOKENS,
            max: tokens,
            default_max: tokens,
        };
    }
    Ok(RouterCatalogEntry {
        router: router.clone(),
        info,
        capabilities,
        context_window_tokens,
        disable_reason: None,
    })
}

pub fn build_router_catalog(
    routers: &[CustomModelRouter],
    providers: &[CustomProviderConfig],
) -> Vec<RouterCatalogEntry> {
    routers
        .iter()
        .map(|router| match router_catalog_entry(router, providers) {
            Ok(entry) => entry,
            Err(error) => {
                let mut info = router.info.clone();
                info.disable_reason = Some(DisableReason::Unavailable);
                RouterCatalogEntry {
                    router: router.clone(),
                    info,
                    capabilities: RouterCatalogCapabilities {
                        chat: false,
                        tools: false,
                        vision: false,
                        embeddings: false,
                        transcription: false,
                    },
                    context_window_tokens: None,
                    disable_reason: Some(error.to_string()),
                }
            }
        })
        .collect()
}

/// Reconcile a persisted active model after a router file or provider setting
/// changed. A stale router never falls back to a hosted model.
pub fn reconcile_active_selection(
    current: &LLMId,
    routers: &[CustomModelRouter],
    providers: &[CustomProviderConfig],
) -> Option<LLMId> {
    if !is_local_custom_router_id(current.as_str()) {
        return Some(current.clone());
    }
    if routers.iter().any(|router| {
        router.llm_id() == *current && router_catalog_entry(router, providers).is_ok()
    }) {
        return Some(current.clone());
    }
    first_concrete_custom_model(providers).map(Into::into)
}

pub fn first_concrete_custom_model(providers: &[CustomProviderConfig]) -> Option<String> {
    providers.iter().find_map(|provider| {
        if !custom_provider_name_is_unique(&provider.name, providers)
            || validate_provider(provider).is_err()
        {
            return None;
        }
        provider.models.iter().find_map(|model| {
            let target = format!("custom/{}/{}", provider.name, model);
            parse_concrete_custom_model_id(&target)
                .is_some()
                .then_some(target)
        })
    })
}

/// Return only concrete, locally configured custom model IDs for router
/// editors. Auto models and ambiguous/invalid providers are never offered.
pub fn concrete_custom_model_ids(providers: &[CustomProviderConfig]) -> Vec<String> {
    let mut models = providers
        .iter()
        .filter(|provider| {
            custom_provider_name_is_unique(&provider.name, providers)
                && validate_provider(provider).is_ok()
        })
        .flat_map(|provider| {
            provider
                .models
                .iter()
                .map(move |model| format!("custom/{}/{}", provider.name, model))
        })
        .filter(|target| parse_concrete_custom_model_id(target).is_some())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

/// A parse error retains the exact file path so a UI can offer an actionable
/// "open/fix this file" affordance without any remote discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelConfigError {
    pub file_name: String,
    pub file_path: PathBuf,
    pub error_message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct YamlCustomModelRouter {
    name: String,
    #[serde(rename = "type")]
    model_type: String,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    routing: serde_yaml::Value,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct YamlComplexityRouting {
    #[serde(default)]
    easy: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    hard: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct YamlPromptRule {
    description: String,
    model: String,
}

#[derive(Serialize)]
struct YamlOutputPromptRule<'a> {
    description: &'a str,
    model: &'a str,
}

#[derive(Serialize)]
struct YamlOutputRouter<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    model_type: &'static str,
    default: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing: Option<serde_yaml::Value>,
}

#[derive(Serialize)]
struct YamlOutputComplexityRouting<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    easy: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    medium: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hard: Option<&'a str>,
}

impl YamlCustomModelRouter {
    fn into_domain(self, source_path: Option<&Path>) -> Result<CustomModelRouter, String> {
        let name = self.name.trim().to_owned();
        if name.is_empty() {
            return Err("custom model router `name` is empty".to_owned());
        }
        let default = self
            .default
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!(
                    "`{name}`: `{}` type requires a `default` model",
                    self.model_type
                )
            })?;
        let routing = match self.model_type.as_str() {
            "complexity" => {
                let routing = if self.routing.is_null() {
                    YamlComplexityRouting::default()
                } else {
                    serde_yaml::from_value(self.routing)
                        .map_err(|error| format!("`{name}`: invalid complexity routing: {error}"))?
                };
                CustomModelRouting::Complexity(ComplexityRouting {
                    default,
                    easy: normalize_target(routing.easy),
                    medium: normalize_target(routing.medium),
                    hard: normalize_target(routing.hard),
                })
            }
            "prompt" => {
                let rules: Vec<YamlPromptRule> = if self.routing.is_null() {
                    Vec::new()
                } else {
                    serde_yaml::from_value(self.routing)
                        .map_err(|error| format!("`{name}`: invalid prompt routing: {error}"))?
                };
                CustomModelRouting::Prompt(PromptRouting {
                    default_model: default,
                    rules: rules
                        .into_iter()
                        .map(|rule| PromptRule {
                            description: rule.description.trim().to_owned(),
                            model: rule.model.trim().to_owned(),
                        })
                        .collect(),
                })
            }
            other => {
                return Err(format!(
                    "`{name}`: unknown type `{other}` (expected `complexity` or `prompt`)"
                ));
            }
        };
        let router = CustomModelRouter::new_local(name, routing, source_path);
        router.validate()?;
        Ok(router)
    }
}

fn normalize_target(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Parse exactly one strict router YAML document.
pub fn parse_model_config_yaml(
    contents: &str,
    source_path: Option<&Path>,
) -> Result<CustomModelRouter, String> {
    if contents.len() > MAX_ROUTER_YAML_BYTES {
        return Err(format!(
            "custom model router YAML exceeds {MAX_ROUTER_YAML_BYTES} bytes"
        ));
    }
    let mut documents = serde_yaml::Deserializer::from_str(contents);
    let document = documents
        .next()
        .ok_or_else(|| "router YAML is empty".to_owned())?;
    let router = YamlCustomModelRouter::deserialize(document)
        .map_err(|error| format!("invalid YAML: {error}"))?;
    if documents.next().is_some() {
        return Err("custom model router YAML must contain exactly one document".to_owned());
    }
    router.into_domain(source_path)
}

fn local_id_from_path(source_path: Option<&Path>, fallback: &str) -> String {
    source_path
        .and_then(Path::file_stem)
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| fallback.to_owned())
}

// ── Atomic local repository ────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterFileRevision {
    pub content_hash: [u8; 32],
    pub size: u64,
    pub modified: Option<SystemTime>,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredCustomModelRouter {
    pub router: CustomModelRouter,
    pub path: PathBuf,
    pub revision: RouterFileRevision,
}

#[derive(Debug, thiserror::Error)]
pub enum LocalCustomModelRouterRepositoryError {
    #[error("router path is outside the managed directory: {path}")]
    InvalidPath { path: PathBuf },
    #[error("router file already exists: {path}")]
    AlreadyExists { path: PathBuf },
    #[error("router file was not found: {path}")]
    NotFound { path: PathBuf },
    #[error("router file changed while it was being edited: {path}")]
    Conflict {
        path: PathBuf,
        expected: RouterFileRevision,
        actual: Option<RouterFileRevision>,
    },
    #[error("router file is not a regular managed file: {path}")]
    NotManaged { path: PathBuf },
    #[error("could not parse router file {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("could not serialize router: {0}")]
    Serialize(#[from] serde_yaml::Error),
    #[error("could not access router file {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

#[derive(Clone, Debug)]
pub struct LocalCustomModelRouterRepository {
    directory: PathBuf,
}

impl LocalCustomModelRouterRepository {
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self {
            directory: directory.as_ref().to_path_buf(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn create(
        &self,
        file_name: impl AsRef<Path>,
        router: &CustomModelRouter,
    ) -> Result<StoredCustomModelRouter, LocalCustomModelRouterRepositoryError> {
        let path = self.managed_path(file_name.as_ref(), true)?;
        self.ensure_directory()?;
        if fs::symlink_metadata(&path).is_ok() {
            return Err(LocalCustomModelRouterRepositoryError::AlreadyExists { path });
        }
        let router = router.with_source_path(&path);
        self.write_atomically(&path, &router, None, false)?;
        self.read(&path)
    }

    pub fn read(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<StoredCustomModelRouter, LocalCustomModelRouterRepositoryError> {
        let path = self.managed_path(path.as_ref(), false)?;
        let _lock = self.lock_directory()?;
        self.read_unlocked(&path)
    }

    fn read_unlocked(
        &self,
        path: &Path,
    ) -> Result<StoredCustomModelRouter, LocalCustomModelRouterRepositoryError> {
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                LocalCustomModelRouterRepositoryError::NotFound {
                    path: path.to_path_buf(),
                }
            } else {
                LocalCustomModelRouterRepositoryError::Io {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LocalCustomModelRouterRepositoryError::NotManaged {
                path: path.to_path_buf(),
            });
        }
        let contents = fs::read_to_string(&path).map_err(|source| {
            LocalCustomModelRouterRepositoryError::Io {
                path: path.to_path_buf(),
                source,
            }
        })?;
        let router = parse_model_config_yaml(&contents, Some(&path)).map_err(|source| {
            LocalCustomModelRouterRepositoryError::Parse {
                path: path.to_path_buf(),
                message: source,
            }
        })?;
        Ok(StoredCustomModelRouter {
            router,
            revision: revision_for(&path, &contents, &metadata),
            path: path.to_path_buf(),
        })
    }

    pub fn update(
        &self,
        path: impl AsRef<Path>,
        expected: &RouterFileRevision,
        router: &CustomModelRouter,
    ) -> Result<StoredCustomModelRouter, LocalCustomModelRouterRepositoryError> {
        let path = self.managed_path(path.as_ref(), false)?;
        let current = self.read(&path)?;
        if &current.revision != expected {
            return Err(LocalCustomModelRouterRepositoryError::Conflict {
                path,
                expected: expected.clone(),
                actual: Some(current.revision),
            });
        }
        let router = router.with_source_path(&path);
        self.write_atomically(&path, &router, Some(expected), true)?;
        self.read(&path)
    }

    pub fn delete(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), LocalCustomModelRouterRepositoryError> {
        let path = self.managed_path(path.as_ref(), false)?;
        let current = self.read(&path)?;
        self.delete_checked(&path, &current.revision)
    }

    /// Delete only when the caller still owns the revision it read.
    pub fn delete_checked(
        &self,
        path: impl AsRef<Path>,
        expected: &RouterFileRevision,
    ) -> Result<(), LocalCustomModelRouterRepositoryError> {
        let path = self.managed_path(path.as_ref(), false)?;
        let _lock = self.lock_directory()?;
        let current = self.read_unlocked(&path)?;
        if &current.revision != expected {
            return Err(LocalCustomModelRouterRepositoryError::Conflict {
                path,
                expected: expected.clone(),
                actual: Some(current.revision),
            });
        }
        fs::remove_file(&path).map_err(|source| LocalCustomModelRouterRepositoryError::Io {
            path: path.clone(),
            source,
        })?;
        if let Some(parent) = path.parent()
            && let Ok(directory) = File::open(parent)
        {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn list(
        &self,
    ) -> Result<Vec<StoredCustomModelRouter>, LocalCustomModelRouterRepositoryError> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(LocalCustomModelRouterRepositoryError::Io {
                    path: self.directory.clone(),
                    source,
                });
            }
        };
        let mut routers = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| LocalCustomModelRouterRepositoryError::Io {
                path: self.directory.clone(),
                source,
            })?;
            let path = entry.path();
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "yaml" | "yml"))
            {
                routers.push(self.read(path)?);
            }
        }
        routers.sort_by(|left, right| {
            left.router
                .info
                .display_name
                .to_lowercase()
                .cmp(&right.router.info.display_name.to_lowercase())
                .then_with(|| left.path.cmp(&right.path))
        });
        Ok(routers)
    }

    fn ensure_directory(&self) -> Result<(), LocalCustomModelRouterRepositoryError> {
        fs::create_dir_all(&self.directory).map_err(|source| {
            LocalCustomModelRouterRepositoryError::Io {
                path: self.directory.clone(),
                source,
            }
        })
    }

    fn lock_directory(&self) -> Result<File, LocalCustomModelRouterRepositoryError> {
        self.ensure_directory()?;
        let path = self.directory.join(".routers.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| LocalCustomModelRouterRepositoryError::Io {
                path: path.clone(),
                source,
            })?;
        #[cfg(unix)]
        flock(file.as_raw_fd(), FlockArg::LockExclusive).map_err(|source| {
            LocalCustomModelRouterRepositoryError::Io {
                path: path.clone(),
                source: io::Error::other(source),
            }
        })?;
        Ok(file)
    }

    fn managed_path(
        &self,
        requested: &Path,
        require_missing_allowed: bool,
    ) -> Result<PathBuf, LocalCustomModelRouterRepositoryError> {
        let directory = if self.directory.exists() {
            fs::canonicalize(&self.directory).map_err(|source| {
                LocalCustomModelRouterRepositoryError::Io {
                    path: self.directory.clone(),
                    source,
                }
            })?
        } else {
            self.directory.clone()
        };
        let path = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            directory.join(requested)
        };
        let Some(file_name) = path.file_name() else {
            return Err(LocalCustomModelRouterRepositoryError::InvalidPath { path });
        };
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
            || file_name.to_string_lossy().starts_with('.')
            || !matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yaml" | "yml")
            )
        {
            return Err(LocalCustomModelRouterRepositoryError::InvalidPath { path });
        }
        let parent = path.parent().unwrap_or(Path::new("."));
        let parent = if parent.exists() {
            fs::canonicalize(parent).map_err(|source| {
                LocalCustomModelRouterRepositoryError::Io {
                    path: parent.to_path_buf(),
                    source,
                }
            })?
        } else {
            parent.to_path_buf()
        };
        if parent != directory {
            return Err(LocalCustomModelRouterRepositoryError::InvalidPath { path });
        }
        if !require_missing_allowed
            && matches!(
                fs::symlink_metadata(&path),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            )
        {
            return Err(LocalCustomModelRouterRepositoryError::NotFound { path });
        }
        if matches!(
            fs::symlink_metadata(&path),
            Ok(metadata) if metadata.file_type().is_symlink()
        ) {
            return Err(LocalCustomModelRouterRepositoryError::InvalidPath { path });
        }
        Ok(directory.join(file_name))
    }

    fn write_atomically(
        &self,
        path: &Path,
        router: &CustomModelRouter,
        expected: Option<&RouterFileRevision>,
        replace_existing: bool,
    ) -> Result<(), LocalCustomModelRouterRepositoryError> {
        let serialized = router.to_yaml_string()?;
        let parent = path.parent().unwrap_or(Path::new("."));
        let _lock = self.lock_directory()?;
        let temporary = parent.join(format!(
            ".{}.tmp-{}",
            path.file_name().unwrap().to_string_lossy(),
            Uuid::new_v4()
        ));
        let result = (|| -> Result<(), LocalCustomModelRouterRepositoryError> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|source| LocalCustomModelRouterRepositoryError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            file.write_all(serialized.as_bytes())
                .and_then(|_| file.flush())
                .and_then(|_| file.sync_all())
                .map_err(|source| LocalCustomModelRouterRepositoryError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            drop(file);
            let actual = if path.exists() {
                let contents = fs::read_to_string(path).map_err(|source| {
                    LocalCustomModelRouterRepositoryError::Io {
                        path: path.to_path_buf(),
                        source,
                    }
                })?;
                let metadata = fs::metadata(path).map_err(|source| {
                    LocalCustomModelRouterRepositoryError::Io {
                        path: path.to_path_buf(),
                        source,
                    }
                })?;
                Some(revision_for(path, &contents, &metadata))
            } else {
                None
            };
            if let Some(expected) = expected {
                if actual.as_ref() != Some(expected) {
                    return Err(LocalCustomModelRouterRepositoryError::Conflict {
                        path: path.to_path_buf(),
                        expected: expected.clone(),
                        actual,
                    });
                }
            } else if actual.is_some() && !replace_existing {
                return Err(LocalCustomModelRouterRepositoryError::AlreadyExists {
                    path: path.to_path_buf(),
                });
            }
            if expected.is_none() && !replace_existing {
                fs::hard_link(&temporary, path).map_err(|source| {
                    if source.kind() == io::ErrorKind::AlreadyExists {
                        LocalCustomModelRouterRepositoryError::AlreadyExists {
                            path: path.to_path_buf(),
                        }
                    } else {
                        LocalCustomModelRouterRepositoryError::Io {
                            path: path.to_path_buf(),
                            source,
                        }
                    }
                })?;
                fs::remove_file(&temporary).map_err(|source| {
                    LocalCustomModelRouterRepositoryError::Io {
                        path: temporary.clone(),
                        source,
                    }
                })?;
            } else {
                fs::rename(&temporary, path).map_err(|source| {
                    LocalCustomModelRouterRepositoryError::Io {
                        path: path.to_path_buf(),
                        source,
                    }
                })?;
            }
            if let Ok(directory) = File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn revision_for(_path: &Path, contents: &str, metadata: &fs::Metadata) -> RouterFileRevision {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    RouterFileRevision {
        content_hash: hasher.finalize().into(),
        size: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        device: std::os::unix::fs::MetadataExt::dev(metadata),
        #[cfg(unix)]
        inode: std::os::unix::fs::MetadataExt::ino(metadata),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::settings::{CustomApiType, CustomProviderCapabilities};

    fn provider(name: &str, models: &[&str]) -> CustomProviderConfig {
        CustomProviderConfig {
            name: name.to_owned(),
            base_url: format!("http://{name}.test/v1"),
            models: models.iter().map(|model| (*model).to_owned()).collect(),
            api_type: CustomApiType::OpenAiCompatible,
            capabilities: CustomProviderCapabilities::default(),
            ..Default::default()
        }
    }

    #[test]
    fn parser_is_strict_and_stable_across_display_name_changes() {
        let path = Path::new("/tmp/routers/release.yaml");
        let yaml = "name: Release helper\ntype: complexity\ndefault: custom/local/code\nrouting:\n  easy: custom/local/fast\n";
        let router = parse_model_config_yaml(yaml, Some(path)).expect("valid router");
        assert_eq!(router.llm_id().as_str(), "custom-router:local:release");
        assert_eq!(
            router.all_targets(),
            ["custom/local/code", "custom/local/fast"]
        );

        let renamed = router.with_display_name("Release assistant".to_owned());
        assert_eq!(renamed.llm_id(), router.llm_id());
        let serialized = renamed.to_yaml_string().expect("serialize router");
        assert!(serialized.contains("name: Release assistant"));
        assert!(parse_model_config_yaml(&serialized, Some(path)).is_ok());
        assert!(
            parse_model_config_yaml(
                "name: bad\ntype: complexity\ndefault: custom/local/code\nextra: true\n",
                Some(path)
            )
            .is_err()
        );
        assert!(parse_model_config_yaml(
            "---\nname: one\ntype: prompt\ndefault: custom/local/code\n---\nname: two\ntype: prompt\ndefault: custom/local/code\n",
            Some(path)
        )
        .is_err());
    }

    #[test]
    fn complexity_routing_is_bounded_and_optional_buckets_fall_back() {
        let router = parse_model_config_yaml(
            "name: Work\ntype: complexity\ndefault: custom/local/medium\nrouting:\n  hard: custom/local/slow\n",
            None,
        )
        .unwrap();
        assert_eq!(
            router
                .resolve(&RouterRequestFacts::default())
                .unwrap()
                .model_id,
            "custom/local/medium"
        );
        let hard = RouterRequestFacts {
            context_chars: 200_000,
            requires_code_review: true,
            ..Default::default()
        };
        assert_eq!(router.resolve(&hard).unwrap().model_id, "custom/local/slow");
    }

    #[test]
    fn prompt_rules_are_ordered_normalized_and_default_to_the_default_model() {
        let router = parse_model_config_yaml(
            "name: Prompt\ntype: prompt\ndefault: custom/local/general\nrouting:\n  - description: \"Rust\"\n    model: custom/local/rust\n  - description: \"кэш\"\n    model: custom/local/cache\n",
            None,
        )
        .unwrap();
        assert_eq!(
            router
                .resolve(&RouterRequestFacts::from_prompt("please use RUST"))
                .unwrap()
                .model_id,
            "custom/local/rust"
        );
        assert_eq!(
            router
                .resolve(&RouterRequestFacts::from_prompt("КЭШ обновить"))
                .unwrap()
                .model_id,
            "custom/local/cache"
        );
        assert_eq!(
            router
                .resolve(&RouterRequestFacts::from_prompt("unrelated request"))
                .unwrap()
                .model_id,
            "custom/local/general"
        );
    }

    #[test]
    fn resolver_rejects_auto_nested_missing_duplicate_invalid_and_capability_mismatch() {
        let providers = vec![provider("local", &["code"]), provider("local", &["other"])];
        let duplicate = parse_model_config_yaml(
            "name: Duplicate\ntype: complexity\ndefault: custom/local/code\n",
            None,
        )
        .unwrap();
        assert!(matches!(
            resolve_router(&duplicate, &RouterRequestFacts::default(), &providers),
            Err(RouterResolutionError::AmbiguousProvider(_))
        ));

        let providers = vec![provider("local", &["code"])];
        for target in ["auto", "custom-router:local:nested", "custom/missing/code"] {
            let yaml = format!("name: Invalid\ntype: complexity\ndefault: {target}\n");
            if let Ok(router) = parse_model_config_yaml(&yaml, None) {
                assert!(
                    resolve_router(&router, &RouterRequestFacts::default(), &providers).is_err()
                );
            }
        }

        let mut invalid = provider("bad", &["code"]);
        invalid.base_url.clear();
        let router = parse_model_config_yaml(
            "name: Invalid\ntype: complexity\ndefault: custom/bad/code\n",
            None,
        )
        .unwrap();
        assert!(matches!(
            resolve_router(&router, &RouterRequestFacts::default(), &[invalid]),
            Err(RouterResolutionError::InvalidProvider { .. })
        ));
    }

    #[test]
    fn catalog_intersects_target_capabilities_and_context_window() {
        let mut first = provider("one", &["fast"]);
        first.capabilities.context_window_tokens = Some(32_000);
        first.capabilities.vision = true;
        let mut second = provider("two", &["safe"]);
        second.capabilities.context_window_tokens = Some(16_000);
        second.capabilities.vision = false;
        let router = parse_model_config_yaml(
            "name: Both\ntype: complexity\ndefault: custom/one/fast\nrouting:\n  hard: custom/two/safe\n",
            None,
        )
        .unwrap();
        let entry = router_catalog_entry(&router, &[first, second]).unwrap();
        assert!(!entry.capabilities.vision);
        assert_eq!(entry.context_window_tokens, Some(16_000));
    }

    #[test]
    fn repository_crud_uses_stable_ids_and_compare_and_swap() {
        let dir = tempfile::tempdir().unwrap();
        let repository = LocalCustomModelRouterRepository::new(dir.path());
        let router = parse_model_config_yaml(
            "name: One\ntype: prompt\ndefault: custom/local/model\n",
            None,
        )
        .unwrap();
        let created = repository.create("one.yaml", &router).unwrap();
        let read = repository.read(&created.path).unwrap();
        assert_eq!(read.router.llm_id(), created.router.llm_id());
        let renamed = router.with_display_name("Renamed".to_owned());
        repository
            .update(&created.path, &read.revision, &renamed)
            .unwrap();
        assert!(matches!(
            repository.update(&created.path, &read.revision, &router),
            Err(LocalCustomModelRouterRepositoryError::Conflict { .. })
        ));
        repository.delete(&created.path).unwrap();
        assert!(repository.list().unwrap().is_empty());
    }
}
