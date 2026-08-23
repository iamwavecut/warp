use parking_lot::FairMutex;
use serde::{Deserialize, Serialize, de};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
};
use warp_core::ui::icons::Icon;
use warp_core::user_preferences::GetUserPreferences;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity};

use crate::workspaces::user_workspaces::{UserWorkspaces, UserWorkspacesEvent};
use crate::{
    ai::agent::api::direct_openai::effective_capabilities_for_config,
    ai::custom_model_routers::{
        CustomModelRouter, RouterCatalogEntry, build_router_catalog, reconcile_active_selection,
    },
    settings::{
        AISettings, CUSTOM_PROVIDER_MIN_CONTEXT_WINDOW_TOKENS, custom_provider_name_is_unique,
    },
};

use ai::api_keys::{ApiKeyManager, ApiKeyManagerEvent};

use super::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::user_config::{WarpConfig, WarpConfigUpdateEvent};

pub use ai::LLMId;

/// Checks if a user's API key is being used for the given provider.
/// Returns `true` if BYO API key is enabled and a key exists for the provider.
pub fn is_using_api_key_for_provider(provider: &LLMProvider, app: &AppContext) -> bool {
    let api_keys = UserWorkspaces::as_ref(app)
        .is_byo_api_key_enabled()
        .then(|| ApiKeyManager::as_ref(app).keys().clone());

    match provider {
        LLMProvider::OpenAI => api_keys.is_some_and(|keys| keys.openai.is_some()),
        LLMProvider::Anthropic => api_keys.is_some_and(|keys| keys.anthropic.is_some()),
        LLMProvider::Google => api_keys.is_some_and(|keys| keys.google.is_some()),
        LLMProvider::Custom(name) => api_keys.is_some_and(|keys| keys.custom.get(name).is_some()),
        _ => false,
    }
}

/// Key for cached LLM metadata in user preferences.
///
/// Note: this key used to store a single [`AvailableLLMs`]
/// but was migrated to store a full [`ModelsByFeature`].
pub const MODELS_BY_FEATURE_CACHE_KEY: &str = "AvailableLLMs";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LLMUsageMetadata {
    pub request_multiplier: usize,
    pub credit_multiplier: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisableReason {
    AdminDisabled,
    OutOfRequests,
    ProviderOutage,
    Unavailable,
}

impl DisableReason {
    /// Returns a user-facing tooltip explaining why the model is disabled.
    pub fn tooltip_text(&self) -> &'static str {
        match self {
            DisableReason::AdminDisabled => "This model is disabled by local provider settings.",
            DisableReason::OutOfRequests => {
                "This hosted quota path is unavailable. Configure a local or BYOK provider."
            }
            DisableReason::ProviderOutage => {
                "This model is temporarily unavailable due to a provider outage."
            }
            DisableReason::Unavailable => "This model is unavailable.",
        }
    }

    /// Returns `true` when this disable reason means the user cannot use the model
    /// and we should clear their stored preference.
    ///
    /// `OutOfRequests` and `ProviderOutage` are transient and expected to
    /// resolve without user action, so we preserve the selection.
    fn should_clear_preference(&self) -> bool {
        match self {
            DisableReason::AdminDisabled | DisableReason::Unavailable => true,
            DisableReason::OutOfRequests | DisableReason::ProviderOutage => false,
        }
    }
}

/// Returns `true` when the model is usable for the current user: not disabled,
/// or disabled for a reason that doesn't block requests (see
/// [`DisableReason::should_clear_preference`]).
fn is_usable_llm(info: &LLMInfo, _app: &AppContext) -> bool {
    info.disable_reason
        .as_ref()
        .is_none_or(|reason| !reason.should_clear_preference())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LLMSpec {
    pub cost: f32,
    pub quality: f32,
    pub speed: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LLMProvider {
    OpenAI,
    Anthropic,
    Google,
    Xai,
    /// A user-configured custom provider (BYOK). The string identifies the provider name.
    Custom(String),
    #[serde(other)]
    Unknown,
}

impl LLMProvider {
    /// Maps an LLMProvider to its corresponding icon.
    pub fn icon(&self) -> Option<Icon> {
        match self {
            LLMProvider::OpenAI => Some(Icon::OpenAILogo),
            LLMProvider::Anthropic => Some(Icon::ClaudeLogo),
            LLMProvider::Google => Some(Icon::GeminiLogo),
            LLMProvider::Xai => None,
            LLMProvider::Custom(_) => None,
            LLMProvider::Unknown => None,
        }
    }
}

/// The host where an LLM can be routed to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LLMModelHost {
    DirectApi,
    AwsBedrock,
    CustomEndpoint,
    #[serde(other)]
    Unknown,
}

/// Configuration for routing an LLM to a specific host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingHostConfig {
    pub enabled: bool,
    pub model_routing_host: LLMModelHost,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMContextWindow {
    #[serde(default)]
    pub is_configurable: bool,
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub max: u32,
    #[serde(default)]
    pub default_max: u32,
}

/// Capabilities that the selected transport can actually deliver for a model.
/// Custom-provider entries use this effective view rather than blindly
/// mirroring user declarations for adapters that are not implemented yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMCapabilities {
    #[serde(default)]
    pub chat: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub embeddings: bool,
    #[serde(default)]
    pub transcription: bool,
}

/// Metadata about an LLM.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LLMInfo {
    pub display_name: String,
    pub base_model_name: String,
    pub id: LLMId,
    pub reasoning_level: Option<String>,
    pub usage_metadata: LLMUsageMetadata,
    pub description: Option<String>,
    pub disable_reason: Option<DisableReason>,
    pub vision_supported: bool,
    pub spec: Option<LLMSpec>,
    pub provider: LLMProvider,
    pub host_configs: HashMap<LLMModelHost, RoutingHostConfig>,
    pub context_window: LLMContextWindow,
    /// Effective transport capabilities. `None` preserves the legacy hosted
    /// metadata shape; custom providers populate this explicitly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<LLMCapabilities>,
}

impl<'de> Deserialize<'de> for LLMInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        /// Helper type that can deserialize host_configs from either:
        /// - A Vec (wire format from server)
        /// - A HashMap (cached format after commit a8a82421c3)
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum HostConfigsWire {
            Vec(Vec<RoutingHostConfig>),
            Map(HashMap<LLMModelHost, RoutingHostConfig>),
        }

        impl Default for HostConfigsWire {
            fn default() -> Self {
                HostConfigsWire::Vec(Vec::new())
            }
        }

        #[derive(Deserialize)]
        struct WireLLMInfo {
            display_name: String,
            #[serde(default)]
            base_model_name: Option<String>,
            id: LLMId,
            #[serde(default)]
            reasoning_level: Option<String>,
            usage_metadata: LLMUsageMetadata,
            #[serde(default)]
            description: Option<String>,
            #[serde(default)]
            disable_reason: Option<DisableReason>,
            #[serde(default)]
            vision_supported: bool,
            #[serde(default)]
            spec: Option<LLMSpec>,
            provider: LLMProvider,
            #[serde(default)]
            host_configs: HostConfigsWire,
            #[serde(default)]
            context_window: LLMContextWindow,
            #[serde(default)]
            capabilities: Option<LLMCapabilities>,
        }

        let wire = WireLLMInfo::deserialize(deserializer)?;
        let host_configs = match wire.host_configs {
            HostConfigsWire::Map(map) => map,
            HostConfigsWire::Vec(vec) => {
                let mut map = HashMap::new();
                for config in vec {
                    let host = config.model_routing_host.clone();
                    if map.insert(host.clone(), config).is_some() {
                        log::warn!(
                            "Duplicate LLMModelHost entry for {:?}, using latest value",
                            host
                        );
                    }
                }
                map
            }
        };
        Ok(Self {
            base_model_name: wire
                .base_model_name
                .unwrap_or_else(|| wire.display_name.clone()),
            vision_supported: wire.vision_supported,
            provider: wire.provider,
            display_name: wire.display_name,
            id: wire.id,
            reasoning_level: wire.reasoning_level,
            usage_metadata: wire.usage_metadata,
            description: wire.description,
            disable_reason: wire.disable_reason,
            spec: wire.spec,
            host_configs,
            context_window: wire.context_window,
            capabilities: wire.capabilities,
        })
    }
}

/// Deduplicates a list of LLMInfo choices by base_model_name and returns an alphabetically sorted
/// list of display names.
pub fn dedupe_model_display_names<'a>(
    choices: impl IntoIterator<Item = &'a LLMInfo>,
) -> Vec<String> {
    let names: HashSet<String> = choices
        .into_iter()
        .map(|choice| choice.base_model_name.clone())
        .collect();
    let mut sorted: Vec<String> = names.into_iter().collect();
    sorted.sort();
    sorted
}

impl LLMInfo {
    /// Returns the display name for the LLM, to be used in the LLM selector menu.
    pub fn menu_display_name(&self) -> String {
        if crate::ai::custom_model_routers::is_custom_router_id(self.id.as_str()) {
            return self.display_name.clone();
        }
        // Base label includes optional description in parentheses
        match &self.description {
            // This is a temporary implementation that won't scale well for longer
            // descriptions. We should implement a better approach for displaying
            // model descriptions, maybe through subtext.
            Some(desc) => format!("{} ({})", self.display_name, desc),
            None => self.display_name.clone(),
        }
    }

    /// Returns the given model's base name.
    /// For non-reasoning models, this is the same as the display name.
    /// E.g. gpt-5.1 (low reasoning) -> gpt-5.1
    pub fn base_model_name(&self) -> &str {
        &self.base_model_name
    }

    /// Returns true if this model has a reasoning level configured.
    pub fn has_reasoning_level(&self) -> bool {
        self.reasoning_level.is_some()
    }

    /// Returns the reasoning level label formatted for display.
    pub fn reasoning_level(&self) -> Option<String> {
        self.reasoning_level.clone()
    }

    #[cfg(feature = "integration_tests")]
    fn new_for_test(llm_name: &str) -> Self {
        Self {
            display_name: llm_name.to_string(),
            base_model_name: llm_name.to_string(),
            id: llm_name.into(),
            reasoning_level: None,
            usage_metadata: LLMUsageMetadata {
                request_multiplier: 1,
                credit_multiplier: None,
            },
            description: None,
            disable_reason: None,
            vision_supported: false, // Default to false for tests
            spec: None,
            provider: LLMProvider::Unknown,
            host_configs: HashMap::new(),
            context_window: LLMContextWindow::default(),
            capabilities: None,
        }
    }
}

/// The set of LLMs available for a feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AvailableLLMs {
    /// The Warp "default" LLM.
    default_id: LLMId,
    choices: Vec<LLMInfo>,

    #[serde(default)]
    preferred_codex_model_id: Option<LLMId>,
}

impl AvailableLLMs {
    /// Constructs an `AvailableLLMs` instance from the given default ID and choices.
    ///
    /// If choices is empty, returns an error.
    ///
    /// If default_id is not a valid ID present in `choices`, takes the first choice in `choices
    /// and uses it as the default.
    pub fn new<T: Into<LLMInfo>>(
        mut default_id: LLMId,
        choices: impl IntoIterator<Item = T>,
        preferred_codex_model_id: Option<LLMId>,
    ) -> Result<Self, anyhow::Error> {
        let choices: Vec<LLMInfo> = choices.into_iter().map(Into::into).collect();
        if choices.is_empty() {
            return Err(anyhow::anyhow!(
                "Tried to create AvailableLLMs with empty`choices`.",
            ));
        } else if !choices.iter().any(|info| info.id == default_id) {
            let fallback_default = choices
                .first()
                .ok_or_else(|| anyhow::anyhow!("Choices should not be empty"))?;
            log::error!(
                "Default LLM ID {} not present in choices, falling back to first choice {}",
                default_id,
                fallback_default.display_name
            );
            default_id = fallback_default.id.clone();
        }

        Ok(Self {
            default_id,
            choices: choices.into_iter().collect(),
            preferred_codex_model_id,
        })
    }

    fn info_for_id(&self, id: &LLMId) -> Option<&LLMInfo> {
        self.choices.iter().find(|info| info.id == *id)
    }

    /// Returns the info for the given id only if the model is usable (present
    /// and not effectively disabled for the current user).
    fn usable_info_for_id(&self, id: &LLMId, app: &AppContext) -> Option<&LLMInfo> {
        self.info_for_id(id).filter(|info| is_usable_llm(info, app))
    }

    fn default_llm_info(&self) -> &LLMInfo {
        if let Some(info) = self.info_for_id(&self.default_id) {
            return info;
        }

        // `new()` enforces that `default_id` is one of `choices`, but
        // deserialization bypasses `new()`, so a stale persisted cache or a
        // server payload can produce an `AvailableLLMs` whose `default_id` is
        // absent from `choices`. Rather than panic, mirror `new()` and fall
        // back to the first choice.
        let fallback = self
            .choices
            .first()
            .expect("AvailableLLMs must have at least one choice");
        log::error!(
            "Default LLM ID {} not present in choices, falling back to first choice {}",
            self.default_id,
            fallback.display_name
        );
        fallback
    }

    fn usable_default_llm_info(&self, app: &AppContext) -> Option<&LLMInfo> {
        self.usable_info_for_id(&self.default_id, app)
            .or_else(|| self.choices.iter().find(|info| is_usable_llm(info, app)))
    }

    #[cfg(feature = "integration_tests")]
    pub fn new_for_test(llm_name: &str) -> Self {
        Self {
            default_id: llm_name.into(),
            choices: vec![LLMInfo::new_for_test(llm_name)],
            preferred_codex_model_id: None,
        }
    }
}

/// The set of models available to the client, grouped by the feature they support.
/// In this fork it is built from local OpenAI-compatible provider settings.
///
/// Currently, if a model is available for multiple features,
/// it will appear denormalized in each of the feature's
/// [`AvailableLLMs`]. While this denormalization doesn't add much value today,
/// it eventually lets us add feature-specific properties to an [`LLMInfo`].
///
/// NOTE: This used to include a `planning` field; this was removed after planning via subagent was
/// deprecated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelsByFeature {
    pub agent_mode: AvailableLLMs,
    pub coding: AvailableLLMs,
    /// The set of LLMs available for CLI agent.
    /// This field is optional during deserialization, as older clients might not have this field.
    #[serde(default)]
    pub cli_agent: Option<AvailableLLMs>,
    /// The set of LLMs available for computer use agent.
    /// This field is optional during deserialization, as older clients might not have this field.
    #[serde(default)]
    pub computer_use: Option<AvailableLLMs>,
}

impl ModelsByFeature {
    /// Returns the info about the LLM identified by `id`, if we have it.
    ///
    /// For models that are available across multiple features,
    /// any one of the metadata will be returned.
    fn info_for_id(&self, id: &LLMId) -> Option<&LLMInfo> {
        self.agent_mode.info_for_id(id)
    }
}

/// Returns the default AvailableLLMs for computer use.
/// Used both in `ModelsByFeature::default()` and as a fallback in `get_computer_use_available()`.
fn default_computer_use_llms() -> AvailableLLMs {
    AvailableLLMs {
        default_id: "computer-use-agent-auto".to_owned().into(),
        choices: vec![LLMInfo {
            display_name: "auto".to_owned(),
            base_model_name: "auto".to_owned(),
            id: "computer-use-agent-auto".to_owned().into(),
            reasoning_level: None,
            usage_metadata: LLMUsageMetadata {
                request_multiplier: 1,
                credit_multiplier: None,
            },
            description: None,
            disable_reason: None,
            vision_supported: true,
            spec: None,
            provider: LLMProvider::Unknown,
            host_configs: HashMap::new(),
            context_window: LLMContextWindow::default(),
            capabilities: None,
        }],
        preferred_codex_model_id: None,
    }
}

impl Default for ModelsByFeature {
    fn default() -> Self {
        Self {
            agent_mode: AvailableLLMs {
                default_id: "auto".to_owned().into(),
                choices: vec![LLMInfo {
                    display_name: "auto (cost-efficient)".to_owned(),
                    base_model_name: "auto (cost-efficient)".to_owned(),
                    id: "auto".to_owned().into(),
                    reasoning_level: None,
                    usage_metadata: LLMUsageMetadata {
                        request_multiplier: 1,
                        credit_multiplier: None,
                    },
                    description: None,
                    disable_reason: None,
                    vision_supported: true,
                    spec: None,
                    provider: LLMProvider::Unknown,
                    host_configs: HashMap::new(),
                    context_window: LLMContextWindow::default(),
                    capabilities: None,
                }],
                preferred_codex_model_id: None,
            },
            coding: AvailableLLMs {
                default_id: "auto".to_owned().into(),
                choices: vec![LLMInfo {
                    display_name: "auto (responsive)".to_owned(),
                    base_model_name: "auto (responsive)".to_owned(),
                    id: "auto".to_owned().into(),
                    reasoning_level: None,
                    usage_metadata: LLMUsageMetadata {
                        request_multiplier: 1,
                        credit_multiplier: None,
                    },
                    description: None,
                    disable_reason: None,
                    vision_supported: true,
                    spec: None,
                    provider: LLMProvider::Unknown,
                    host_configs: HashMap::new(),
                    context_window: LLMContextWindow::default(),
                    capabilities: None,
                }],
                preferred_codex_model_id: None,
            },
            cli_agent: Some(AvailableLLMs {
                default_id: "cli-agent-auto".to_owned().into(),
                choices: vec![LLMInfo {
                    display_name: "auto".to_owned(),
                    base_model_name: "auto".to_owned(),
                    id: "cli-agent-auto".to_owned().into(),
                    reasoning_level: None,
                    usage_metadata: LLMUsageMetadata {
                        request_multiplier: 1,
                        credit_multiplier: None,
                    },
                    description: None,
                    disable_reason: None,
                    vision_supported: false,
                    spec: None,
                    provider: LLMProvider::Unknown,
                    host_configs: HashMap::new(),
                    context_window: LLMContextWindow::default(),
                    capabilities: None,
                }],
                preferred_codex_model_id: None,
            }),
            computer_use: Some(default_computer_use_llms()),
        }
    }
}

enum UpdatePopupVisibilityState {
    WaitingToBeShown,
    Visible(EntityId),
    Hidden,
}

struct AvailableLLMsUpdate {
    new_choices: Vec<LLMInfo>,
    popup_visibility_state: Arc<FairMutex<UpdatePopupVisibilityState>>,
}

/// Singleton model holding user/workspace LLM preferences, including the set of LLMs available for
/// use as well as the user's preferred LLM for Agent Mode.
pub struct LLMPreferences {
    models_by_feature: ModelsByFeature,
    last_update: Option<AvailableLLMsUpdate>,
    // Stores temporary model overrides for a given terminal view.
    // NOTE: We only store an override if the model selected by the user is different
    // from the base LLM for the active profile. This means that if the user selects the
    // profile's default model and changes their profile, the model will update to that profile's default.
    base_llm_for_terminal_view: HashMap<EntityId, LLMId>,
    /// Local YAML-authored routers exposed as picker entries.
    custom_model_routers: Vec<CustomModelRouter>,
}

impl LLMPreferences {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Build the startup catalog directly from local settings. Reading the persisted model
        // cache here could expose stale custom or hosted models before the first Agent Pane opens.
        let models_by_feature = models_by_feature_from_custom_providers(ctx);

        ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), |me, _, event, ctx| {
            if let UserWorkspacesEvent::TeamsChanged = event {
                me.refresh_available_models(ctx);
            }
        });

        // Re-reconcile disabled model preferences when BYOK keys change, since
        // provider availability can change with local key configuration.
        ctx.subscribe_to_model(
            &ApiKeyManager::handle(ctx),
            |me, _, _event: &ApiKeyManagerEvent, ctx| {
                me.reconcile_disabled_model_preferences(ctx);
            },
        );

        #[cfg(not(test))]
        ctx.subscribe_to_model(&WarpConfig::handle(ctx), |me, _, event, ctx| {
            if matches!(event, WarpConfigUpdateEvent::ModelConfigs) {
                me.rebuild_custom_model_routers(ctx);
                me.reconcile_disabled_model_preferences(ctx);
                ctx.emit(LLMPreferencesEvent::UpdatedAvailableLLMs);
            }
        });

        let base_llm_for_terminal_view = HashMap::new();

        let mut me = Self {
            models_by_feature,
            last_update: None,
            base_llm_for_terminal_view,
            custom_model_routers: Vec::new(),
        };

        #[cfg(not(test))]
        me.rebuild_custom_model_routers(ctx);
        me
    }

    /// Returns the `LLMInfo` for the base LLM to be used for an Agent Mode request.
    pub fn get_active_base_model<'a>(
        &'a self,
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        self.get_preferred_base_model(app, terminal_view_id)
    }

    /// Returns `LLMInfo` for the currently selected LLM to be used for Agent Mode.
    fn get_preferred_base_model(
        &self,
        app: &AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &LLMInfo {
        if let Some(terminal_view_id) = terminal_view_id {
            let raw_override = self.base_llm_for_terminal_view.get(&terminal_view_id);
            if let Some(llm_id) = raw_override
                && let Some(llm_info) =
                    self.model_info_for_id(&self.models_by_feature.agent_mode, llm_id, app)
            {
                return llm_info;
            }
        }

        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);

        profile
            .data()
            .base_model
            .clone()
            .and_then(|id| self.model_info_for_id(&self.models_by_feature.agent_mode, &id, app))
            .unwrap_or_else(|| self.fallback_llm_info(&self.models_by_feature.agent_mode, app))
    }

    pub fn get_active_coding_model<'a>(
        &'a self,
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        self.get_preferred_coding_model(app, terminal_view_id)
    }

    /// Returns `LLMInfo` for user's preferred coding model.
    fn get_preferred_coding_model(
        &self,
        app: &AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &LLMInfo {
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);

        profile
            .data()
            .coding_model
            .clone()
            .and_then(|id| self.model_info_for_id(&self.models_by_feature.coding, &id, app))
            .unwrap_or_else(|| self.fallback_llm_info(&self.models_by_feature.coding, app))
    }

    /// Returns the set of LLMs available for Agent Mode use.
    pub fn get_base_llm_choices_for_agent_mode(&self) -> impl Iterator<Item = &LLMInfo> {
        // Don't show admin-disabled models in the dropdown
        self.models_by_feature
            .agent_mode
            .choices
            .iter()
            .filter(|llm| !matches!(llm.disable_reason, Some(DisableReason::AdminDisabled)))
            .chain(self.custom_router_choices())
    }

    /// Returns the set of LLMs available for coding.
    pub fn get_coding_llm_choices(&self) -> impl Iterator<Item = &LLMInfo> {
        // Don't show admin-disabled models in the dropdown
        self.models_by_feature
            .coding
            .choices
            .iter()
            .filter(|llm| !matches!(llm.disable_reason, Some(DisableReason::AdminDisabled)))
            .chain(self.custom_router_choices())
    }

    /// Returns the set of LLMs available for CLI agent.
    pub fn get_cli_agent_llm_choices(&self) -> impl Iterator<Item = &LLMInfo> {
        self.get_cli_agent_available().choices.iter()
    }

    /// Returns the `LLMInfo` for the CLI agent model.
    pub fn get_active_cli_agent_model<'a>(
        &'a self,
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);

        let available = self.get_cli_agent_available();
        profile
            .data()
            .cli_agent_model
            .clone()
            .and_then(|id| available.usable_info_for_id(&id, app))
            .unwrap_or_else(|| self.get_default_cli_agent_model(app))
    }

    /// Returns the effective default CLI agent model as a fallback
    /// (disable-aware, see [`Self::fallback_llm_info`]).
    pub fn get_default_cli_agent_model(&self, app: &AppContext) -> &LLMInfo {
        self.fallback_llm_info(self.get_cli_agent_available(), app)
    }

    /// Helper to get the AvailableLLMs for cli_agent, falling back to agent_mode.
    fn get_cli_agent_available(&self) -> &AvailableLLMs {
        self.models_by_feature
            .cli_agent
            .as_ref()
            .unwrap_or(&self.models_by_feature.agent_mode)
    }

    /// Returns the set of LLMs available for computer use agent.
    pub fn get_computer_use_llm_choices(&self) -> impl Iterator<Item = &LLMInfo> {
        self.get_computer_use_available().choices.iter()
    }

    /// Returns the `LLMInfo` for the computer use agent model.
    pub fn get_active_computer_use_model<'a>(
        &'a self,
        app: &'a AppContext,
        terminal_view_id: Option<EntityId>,
    ) -> &'a LLMInfo {
        let profile = AIExecutionProfilesModel::as_ref(app).active_profile(terminal_view_id, app);

        let available = self.get_computer_use_available();
        profile
            .data()
            .computer_use_model
            .clone()
            .and_then(|id| available.usable_info_for_id(&id, app))
            .unwrap_or_else(|| self.get_default_computer_use_model(app))
    }

    /// Returns the effective default computer use model as a fallback: the
    /// server default when usable, else the first usable choice, else the
    /// (possibly disabled) server default. No custom-endpoint fallback here:
    /// custom models aren't offered for computer use.
    pub fn get_default_computer_use_model(&self, app: &AppContext) -> &LLMInfo {
        let available = self.get_computer_use_available();
        available
            .usable_default_llm_info(app)
            .unwrap_or_else(|| available.default_llm_info())
    }

    /// Helper to get the AvailableLLMs for computer_use.
    /// Falls back to a computer-use-specific default if None.
    fn get_computer_use_available(&self) -> &AvailableLLMs {
        static DEFAULT: OnceLock<AvailableLLMs> = OnceLock::new();
        self.models_by_feature
            .computer_use
            .as_ref()
            .unwrap_or_else(|| DEFAULT.get_or_init(default_computer_use_llms))
    }

    /// Returns metadata about an LLM, if the client knows about it.
    pub fn get_llm_info(&self, id: &LLMId) -> Option<&LLMInfo> {
        self.models_by_feature
            .info_for_id(id)
            .or_else(|| self.custom_router_llm_info_for_id(id))
    }

    /// Resolve a local router by its stable filename-derived picker id.
    pub fn custom_model_router_for_id(&self, id: &LLMId) -> Option<&CustomModelRouter> {
        self.custom_model_routers
            .iter()
            .find(|router| router.llm_id() == *id)
    }

    /// Return local router entries which have a fully configured concrete
    /// target catalog. Invalid routers remain available to the settings error
    /// surface, but are never selectable in model pickers.
    pub fn custom_router_choices(&self) -> impl Iterator<Item = &LLMInfo> {
        self.custom_model_routers
            .iter()
            .filter(|router| router.info.disable_reason.is_none())
            .map(|router| &router.info)
    }

    fn custom_router_llm_info_for_id(&self, id: &LLMId) -> Option<&LLMInfo> {
        self.custom_model_router_for_id(id)
            .map(|router| &router.info)
            .filter(|info| info.disable_reason.is_none())
    }

    fn model_info_for_id<'a>(
        &'a self,
        available: &'a AvailableLLMs,
        id: &LLMId,
        app: &AppContext,
    ) -> Option<&'a LLMInfo> {
        available
            .usable_info_for_id(id, app)
            .or_else(|| self.custom_router_llm_info_for_id(id))
    }

    fn custom_llm_info_for_id_if_enabled(&self, id: &LLMId, app: &AppContext) -> Option<&LLMInfo> {
        self.models_by_feature
            .agent_mode
            .usable_info_for_id(id, app)
            .filter(|info| matches!(info.provider, LLMProvider::Custom(_)))
    }

    /// Disable-aware fallback for local-first model selection.
    fn fallback_llm_info<'a>(
        &'a self,
        available: &'a AvailableLLMs,
        app: &AppContext,
    ) -> &'a LLMInfo {
        available
            .usable_default_llm_info(app)
            .or_else(|| {
                self.models_by_feature
                    .agent_mode
                    .choices
                    .iter()
                    .find(|info| {
                        matches!(info.provider, LLMProvider::Custom(_)) && is_usable_llm(info, app)
                    })
            })
            .unwrap_or_else(|| available.default_llm_info())
    }

    /// Returns the effective default base model as a fallback
    /// (disable-aware, see [`Self::fallback_llm_info`]).
    pub fn get_default_base_model(&self, app: &AppContext) -> &LLMInfo {
        self.fallback_llm_info(&self.models_by_feature.agent_mode, app)
    }

    /// Returns the effective default coding model as a fallback
    /// (disable-aware, see [`Self::fallback_llm_info`]).
    pub fn get_default_coding_model(&self, app: &AppContext) -> &LLMInfo {
        self.fallback_llm_info(&self.models_by_feature.coding, app)
    }

    /// Returns the preferred Codex model, if set by the server.
    pub fn get_preferred_codex_model(&self) -> Option<&LLMInfo> {
        self.models_by_feature
            .agent_mode
            .preferred_codex_model_id
            .as_ref()
            .and_then(|id| self.models_by_feature.agent_mode.info_for_id(id))
    }

    #[cfg(feature = "integration_tests")]
    pub fn is_available_agent_mode_llm(&self, id: &LLMId) -> bool {
        self.models_by_feature.agent_mode.info_for_id(id).is_some()
    }

    /// Creates a pane-level override for the Agent Mode LLM.
    pub fn update_preferred_agent_mode_llm(
        &mut self,
        preferred_llm_id: &LLMId,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let profile =
            AIExecutionProfilesModel::as_ref(ctx).active_profile(Some(terminal_view_id), ctx);

        let profile_default_model_id = profile
            .data()
            .base_model
            .as_ref()
            .and_then(|id| self.models_by_feature.agent_mode.info_for_id(id))
            .unwrap_or_else(|| self.models_by_feature.agent_mode.default_llm_info())
            .id
            .clone();

        // Only remove override if we're setting to the profile's default.
        // Otherwise, always set the override explicitly.
        let changed = if preferred_llm_id == &profile_default_model_id {
            self.base_llm_for_terminal_view
                .remove(&terminal_view_id)
                .is_some()
        } else {
            self.base_llm_for_terminal_view
                .insert(terminal_view_id, preferred_llm_id.clone());
            true
        };

        if changed {
            self.trigger_snapshot_save(ctx);
            ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
        }
    }

    /// Copies the raw per-pane Agent Mode override from one terminal view to
    /// another, preserving the source pane's local model selection exactly.
    pub(crate) fn copy_agent_mode_selection(
        &mut self,
        source_terminal_view_id: EntityId,
        new_terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed = match self
            .base_llm_for_terminal_view
            .get(&source_terminal_view_id)
            .cloned()
        {
            Some(id) => {
                self.base_llm_for_terminal_view
                    .insert(new_terminal_view_id, id.clone())
                    != Some(id)
            }
            None => self
                .base_llm_for_terminal_view
                .remove(&new_terminal_view_id)
                .is_some(),
        };

        if changed {
            self.trigger_snapshot_save(ctx);
            ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
        }
    }

    /// Triggers a snapshot save to persist LLM override changes.
    fn trigger_snapshot_save(&self, ctx: &mut ModelContext<Self>) {
        ctx.dispatch_global_action("workspace:save_app", ());
    }

    pub fn update_preferred_coding_llm(
        &self,
        preferred_llm_id: &LLMId,
        terminal_view_id: Option<EntityId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let new_value = if preferred_llm_id == &self.models_by_feature.coding.default_id {
            None
        } else {
            Some(preferred_llm_id.clone())
        };

        let mut changed = false;
        AIExecutionProfilesModel::handle(ctx).update(ctx, |profiles, ctx| {
            let profile = profiles.active_profile(terminal_view_id, ctx);

            if profile.data().coding_model != new_value {
                profiles.set_coding_model(*profile.id(), new_value, ctx);
                changed = true;
            }
        });

        if changed {
            ctx.emit(LLMPreferencesEvent::UpdatedActiveCodingLLM);
        }
    }

    pub fn new_choices_since_last_update(&self) -> Option<Vec<LLMInfo>> {
        self.last_update.as_ref().map(|update| {
            // We don't want to display new choices if they are warp branded.
            let filter_choices: Vec<LLMInfo> = update
                .new_choices
                .clone()
                .into_iter()
                .filter(|choice| !choice.display_name.starts_with("lite"))
                .collect();

            filter_choices
        })
    }

    pub fn should_show_new_choices_popup(&self, view_id: EntityId) -> bool {
        self.last_update.as_ref().is_some_and(|update| {
            let popup_state = &*update.popup_visibility_state.lock();
            matches!(popup_state, UpdatePopupVisibilityState::WaitingToBeShown)
                || matches!(
                popup_state,
                UpdatePopupVisibilityState::Visible(id) if *id == view_id)
        })
    }

    pub fn mark_new_choices_popup_as_shown(&self, view_id: EntityId) {
        if let Some(update) = self.last_update.as_ref()
            && matches!(
                &*update.popup_visibility_state.lock(),
                UpdatePopupVisibilityState::WaitingToBeShown
            )
        {
            *update.popup_visibility_state.lock() = UpdatePopupVisibilityState::Visible(view_id);
        }
    }

    pub fn hide_llm_popup(&self, view_id: EntityId) {
        if !self.should_show_new_choices_popup(view_id) {
            return;
        }
        let Some(last_update) = self.last_update.as_ref() else {
            return;
        };
        *last_update.popup_visibility_state.lock() = UpdatePopupVisibilityState::Hidden;
    }

    pub fn refresh_available_models(&mut self, ctx: &mut ModelContext<Self>) {
        self.refresh_custom_provider_models(ctx);
        #[cfg(not(test))]
        self.rebuild_custom_model_routers(ctx);
    }

    fn rebuild_custom_model_routers(&mut self, ctx: &mut ModelContext<Self>) {
        let routers = WarpConfig::as_ref(ctx).custom_model_routers().to_vec();
        let providers = AISettings::as_ref(ctx).custom_providers.clone();
        let catalog = build_router_catalog(&routers, &providers);

        self.custom_model_routers = catalog
            .into_iter()
            .map(|RouterCatalogEntry { router, info, .. }| {
                let mut router = router;
                router.info = info;
                router
            })
            .collect();

        let reconciled = self
            .base_llm_for_terminal_view
            .iter()
            .map(|(view_id, selected)| {
                (
                    *view_id,
                    reconcile_active_selection(selected, &self.custom_model_routers, &providers),
                )
            })
            .collect::<Vec<_>>();
        for (view_id, selected) in reconciled {
            if let Some(selected) = selected {
                self.base_llm_for_terminal_view.insert(view_id, selected);
            } else {
                self.base_llm_for_terminal_view.remove(&view_id);
            }
        }
    }

    /// Build the model list from custom provider configs stored in AISettings
    /// instead of fetching hosted model metadata from Warp servers.
    fn refresh_custom_provider_models(&self, ctx: &mut ModelContext<Self>) {
        let models_by_feature = models_by_feature_from_custom_providers(ctx);

        // Use ctx.spawn with an immediately-resolving future so we get
        // &mut Self access in the callback.
        let update = models_by_feature;
        ctx.spawn(
            async move { Ok::<_, anyhow::Error>(update) },
            |me, result, ctx| {
                if let Ok(update) = result
                    && update != me.models_by_feature
                {
                    me.on_server_update(update, ctx);
                }
            },
        );
    }

    fn on_server_update(&mut self, update: ModelsByFeature, ctx: &mut ModelContext<Self>) {
        let has_existing_persisted_config = get_cached_models(ctx).is_some();

        let old = std::mem::replace(&mut self.models_by_feature, update);

        match serde_json::to_string(&self.models_by_feature) {
            Ok(serialized_update) => {
                if let Err(e) = ctx
                    .private_user_preferences()
                    .write_value(MODELS_BY_FEATURE_CACHE_KEY, serialized_update)
                {
                    log::error!("Failed to cache LLMs: {e}");
                }
            }
            Err(e) => {
                log::error!("Failed to serialize LLMs for cache: {e}");
            }
        }

        self.reconcile_disabled_model_preferences(ctx);

        let new_choices =
            get_new_agent_mode_choices(&old.agent_mode, &self.models_by_feature.agent_mode);
        if !new_choices.is_empty() {
            self.last_update = Some(AvailableLLMsUpdate {
                new_choices,
                // We shouldn't show the update for the initial LLM config creation.
                popup_visibility_state: Arc::new(FairMutex::new(
                    if has_existing_persisted_config {
                        UpdatePopupVisibilityState::WaitingToBeShown
                    } else {
                        UpdatePopupVisibilityState::Hidden
                    },
                )),
            });
        }

        ctx.emit(LLMPreferencesEvent::UpdatedAvailableLLMs);
    }

    /// Clear any model selections where the model is no longer supported
    /// or effectively disabled, and clear orphaned context window limits
    /// for non-configurable or unusable models.
    ///
    /// Called both when the local model list is refreshed and when BYOK API keys change.
    fn reconcile_disabled_model_preferences(&self, ctx: &mut ModelContext<Self>) {
        let profiles_model = AIExecutionProfilesModel::handle(ctx);
        profiles_model.update(ctx, |profiles, ctx| {
            for profile_id in profiles.get_all_profile_ids() {
                if let Some(profile) = profiles.get_profile_by_id(profile_id, ctx) {
                    let profile_data = profile.data();
                    let preferred_base_model = profile_data.base_model.clone();
                    let effective_base_model_id = preferred_base_model
                        .as_ref()
                        .unwrap_or(&self.models_by_feature.agent_mode.default_id);
                    let effective_base_model_usable = self
                        .models_by_feature
                        .agent_mode
                        .usable_info_for_id(effective_base_model_id, ctx)
                        .or_else(|| {
                            self.custom_llm_info_for_id_if_enabled(effective_base_model_id, ctx)
                        });
                    let effective_base_model_usable = effective_base_model_usable
                        .or_else(|| self.custom_router_llm_info_for_id(effective_base_model_id));
                    let effective_base_model_unusable = effective_base_model_usable.is_none();
                    let effective_base_model_is_configurable = effective_base_model_usable
                        .is_some_and(|info| info.context_window.is_configurable);
                    let has_context_window_limit = profile_data.context_window_limit.is_some();

                    if preferred_base_model.is_some() && effective_base_model_unusable {
                        profiles.set_base_model(profile_id, None, ctx);
                    }
                    if has_context_window_limit
                        && (effective_base_model_unusable || !effective_base_model_is_configurable)
                    {
                        profiles.set_context_window_limit(profile_id, None, ctx);
                    }
                    if let Some(preferred_llm_id) = &profile.data().coding_model
                        && self
                            .models_by_feature
                            .coding
                            .usable_info_for_id(preferred_llm_id, ctx)
                            .or_else(|| {
                                self.custom_llm_info_for_id_if_enabled(preferred_llm_id, ctx)
                            })
                            .or_else(|| self.custom_router_llm_info_for_id(preferred_llm_id))
                            .is_none()
                    {
                        profiles.set_coding_model(profile_id, None, ctx);
                    }
                    if let Some(preferred_llm_id) = &profile.data().cli_agent_model
                        && self
                            .get_cli_agent_available()
                            .usable_info_for_id(preferred_llm_id, ctx)
                            .or_else(|| {
                                self.custom_llm_info_for_id_if_enabled(preferred_llm_id, ctx)
                            })
                            .or_else(|| self.custom_router_llm_info_for_id(preferred_llm_id))
                            .is_none()
                    {
                        profiles.set_cli_agent_model(profile_id, None, ctx);
                    }
                    if let Some(preferred_llm_id) = &profile.data().computer_use_model
                        && self
                            .get_computer_use_available()
                            .usable_info_for_id(preferred_llm_id, ctx)
                            .is_none()
                    {
                        profiles.set_computer_use_model(profile_id, None, ctx);
                    }
                }
            }
        });
    }

    pub fn vision_supported(&self, app: &AppContext, terminal_view_id: Option<EntityId>) -> bool {
        self.get_active_base_model(app, terminal_view_id)
            .vision_supported
    }

    pub fn get_base_llm_override(&self, terminal_view_id: EntityId) -> Option<String> {
        if let Some(override_str) = self
            .base_llm_for_terminal_view
            .get(&terminal_view_id)
            .and_then(|llm_id| serde_json::to_string(llm_id).ok())
        {
            return Some(override_str);
        }

        log::debug!("LLM override not found in memory for terminal view: {terminal_view_id:?}");
        None
    }

    /// Removes the LLM override for a terminal view.
    /// This ensures that the new profile's default model is used.
    pub fn remove_llm_override(
        &mut self,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let old = self.base_llm_for_terminal_view.remove(&terminal_view_id);
        if old.is_some() {
            self.trigger_snapshot_save(ctx);
            ctx.emit(LLMPreferencesEvent::UpdatedActiveAgentModeLLM);
        }
    }

    #[cfg(test)]
    pub fn update_models_for_testing(
        &mut self,
        update: ModelsByFeature,
        ctx: &mut ModelContext<Self>,
    ) {
        self.on_server_update(update, ctx);
    }
}

#[derive(Clone, Debug)]
pub enum LLMPreferencesEvent {
    UpdatedAvailableLLMs,
    UpdatedActiveAgentModeLLM,
    UpdatedActiveCodingLLM,
}

impl Entity for LLMPreferences {
    type Event = LLMPreferencesEvent;
}

impl SingletonEntity for LLMPreferences {}

/// Builds the locally configured model catalog, or the explicit local fallback when no custom
/// provider has models. This is pure settings-to-model transformation and performs no I/O.
fn models_by_feature_from_custom_providers(app: &AppContext) -> ModelsByFeature {
    let mut all_llms = Vec::new();
    let providers = AISettings::as_ref(app).custom_providers.as_slice();
    for provider_config in providers {
        if !custom_provider_name_is_unique(&provider_config.name, providers) {
            log::warn!(
                "Skipping ambiguous local custom provider `{}`; rename one duplicate provider",
                provider_config.name
            );
            continue;
        }
        if let Err(error) = provider_config.validate() {
            log::warn!(
                "Skipping invalid local custom provider `{}`: {error}",
                provider_config.name
            );
            continue;
        }
        let provider_name = provider_config.name.clone();
        let provider = LLMProvider::Custom(provider_name.clone());
        let effective_capabilities =
            effective_capabilities_for_config(&provider_config.capabilities);
        let context_window = provider_config
            .capabilities
            .context_window_tokens
            .map(|tokens| LLMContextWindow {
                // The provider-level limit is consumed by the direct adapter's
                // context truncation boundary. Execution-profile slider
                // values are not threaded into RequestParams yet, so do not
                // advertise a second, non-functional profile control here.
                is_configurable: false,
                min: CUSTOM_PROVIDER_MIN_CONTEXT_WINDOW_TOKENS,
                max: tokens,
                default_max: tokens,
            })
            .unwrap_or_default();

        for model_id in &provider_config.models {
            all_llms.push(LLMInfo {
                display_name: format!("{} / {}", provider_name, model_id),
                base_model_name: model_id.clone(),
                id: format!("custom/{}/{}", provider_name, model_id).into(),
                reasoning_level: None,
                usage_metadata: LLMUsageMetadata {
                    request_multiplier: 1,
                    credit_multiplier: None,
                },
                description: None,
                disable_reason: None,
                vision_supported: effective_capabilities.vision,
                spec: None,
                provider: provider.clone(),
                host_configs: HashMap::new(),
                context_window: context_window.clone(),
                capabilities: Some(LLMCapabilities {
                    chat: effective_capabilities.chat,
                    tools: effective_capabilities.tools,
                    vision: effective_capabilities.vision,
                    embeddings: effective_capabilities.embeddings,
                    transcription: effective_capabilities.transcription,
                }),
            });
        }
    }

    let Some(default_id) = all_llms.first().map(|llm| llm.id.clone()) else {
        log::info!("local-first: no custom provider models configured, using local fallback");
        return ModelsByFeature::default();
    };

    let available = AvailableLLMs::new(default_id, all_llms, None)
        .expect("custom provider model list is non-empty");

    ModelsByFeature {
        agent_mode: available.clone(),
        coding: available.clone(),
        cli_agent: Some(available),
        computer_use: Some(default_computer_use_llms()),
    }
}

fn get_new_agent_mode_choices(
    old_config: &AvailableLLMs,
    new_config: &AvailableLLMs,
) -> Vec<LLMInfo> {
    let old_ids: HashSet<_> = old_config.choices.iter().map(|info| &info.id).collect();
    new_config
        .choices
        .iter()
        .filter(|info| !old_ids.contains(&info.id))
        .cloned()
        .collect()
}

/// Gets the last cached LLM metadata.
fn get_cached_models(app: &mut AppContext) -> Option<ModelsByFeature> {
    let value = app
        .private_user_preferences()
        .read_value(MODELS_BY_FEATURE_CACHE_KEY)
        .ok()
        .flatten()?;

    // Try to deserialize to the [`ModelsByFeature`] type.
    match serde_json::from_str::<ModelsByFeature>(value.as_str()) {
        Ok(config) => Some(config),
        Err(e1) => {
            // If that fails, try to deserialize directly to [`AvailableLLMs`].
            // Before we had model choice by feature, all available LLMs were solely
            // for Agent Mode.
            match serde_json::from_str::<AvailableLLMs>(value.as_str()) {
                Ok(config) => Some(ModelsByFeature {
                    agent_mode: config,
                    ..Default::default()
                }),
                Err(e2) => {
                    log::warn!("Failed to deserialize cached LLMs: {e1}\n{e2}");
                    None
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "llms_tests.rs"]
mod tests;
