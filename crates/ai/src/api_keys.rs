pub use crate::aws_credentials::{AwsCredentials, AwsCredentialsState};
use serde::{Deserialize, Serialize};
use warp_multi_agent_api as api;
use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

const SECURE_STORAGE_KEY: &str = "AiApiKeys";

/// Emitted when user-provided API keys are updated in-memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyManagerEvent {
    KeysUpdated,
}

/// User-provided API keys for AI providers.
///
/// These are used for "Bring Your Own API Key" functionality, allowing
/// users to use their own API keys instead of Warp's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ApiKeys {
    pub google: Option<String>,
    pub anthropic: Option<String>,
    pub openai: Option<String>,
    pub open_router: Option<String>,
    /// Custom provider API keys, keyed by provider name.
    #[serde(default)]
    pub custom: std::collections::HashMap<String, String>,
}

impl ApiKeys {
    pub fn has_any_key(&self) -> bool {
        self.openai.is_some()
            || self.anthropic.is_some()
            || self.google.is_some()
            || self.open_router.is_some()
            || !self.custom.is_empty()
    }
}

/// A structure that manages API keys for AI providers.
pub struct ApiKeyManager {
    keys: ApiKeys,
    pub(crate) aws_credentials_state: AwsCredentialsState,
    startup_keys_mutated: bool,
}

impl ApiKeyManager {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let keys = Self::load_keys_from_secure_storage(ctx);
        Self::with_keys(keys)
    }

    #[cfg(target_os = "macos")]
    pub fn new_deferred(service_name: String, ctx: &mut ModelContext<Self>) -> Self {
        let manager = Self::with_keys(ApiKeys::default());
        ctx.spawn(
            async move { Self::load_keys_from_named_secure_storage(&service_name) },
            |manager, keys, ctx| {
                if manager.apply_startup_loaded_keys(keys) {
                    ctx.emit(ApiKeyManagerEvent::KeysUpdated);
                }
            },
        );
        manager
    }

    fn with_keys(keys: ApiKeys) -> Self {
        Self {
            keys,
            aws_credentials_state: AwsCredentialsState::Missing,
            startup_keys_mutated: false,
        }
    }

    pub fn keys(&self) -> &ApiKeys {
        &self.keys
    }

    pub fn set_google_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.startup_keys_mutated = true;
        self.keys.google = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_anthropic_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.startup_keys_mutated = true;
        self.keys.anthropic = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_openai_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.startup_keys_mutated = true;
        self.keys.openai = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_open_router_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.startup_keys_mutated = true;
        self.keys.open_router = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    /// Sets or removes a custom provider API key.
    pub fn set_custom_key(
        &mut self,
        provider_name: String,
        key: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.startup_keys_mutated = true;
        match key {
            Some(k) => {
                self.keys.custom.insert(provider_name, k);
            }
            None => {
                self.keys.custom.remove(&provider_name);
            }
        }
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_aws_credentials_state(
        &mut self,
        state: AwsCredentialsState,
        ctx: &mut ModelContext<Self>,
    ) {
        self.aws_credentials_state = state;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
    }

    pub fn aws_credentials_state(&self) -> &AwsCredentialsState {
        &self.aws_credentials_state
    }

    pub fn api_keys_for_request(
        &self,
        include_byo_keys: bool,
        include_aws_bedrock_credentials: bool,
    ) -> Option<api::request::settings::ApiKeys> {
        let anthropic = include_byo_keys
            .then(|| self.keys.anthropic.clone())
            .flatten()
            .unwrap_or_default();
        let openai = include_byo_keys
            .then(|| self.keys.openai.clone())
            .flatten()
            .unwrap_or_default();
        let google = include_byo_keys
            .then(|| self.keys.google.clone())
            .flatten()
            .unwrap_or_default();
        let open_router = include_byo_keys
            .then(|| self.keys.open_router.clone())
            .flatten()
            .unwrap_or_default();
        let aws_credentials = include_aws_bedrock_credentials
            .then(|| match self.aws_credentials_state {
                AwsCredentialsState::Loaded {
                    ref credentials, ..
                } => Some(credentials.clone().into()),
                _ => None,
            })
            .flatten();

        if anthropic.is_empty()
            && openai.is_empty()
            && google.is_empty()
            && open_router.is_empty()
            && aws_credentials.is_none()
        {
            None
        } else {
            Some(api::request::settings::ApiKeys {
                anthropic,
                openai,
                google,
                open_router,
                allow_use_of_warp_credits: false,
                aws_credentials,
            })
        }
    }

    fn load_keys_from_secure_storage(ctx: &mut ModelContext<Self>) -> ApiKeys {
        Self::deserialize_keys(match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(json) => json,
            Err(e) => {
                if !matches!(e, secure_storage::Error::NotFound) {
                    log::error!("Failed to read API keys from secure storage: {e:#}");
                }
                return ApiKeys::default();
            }
        })
    }

    #[cfg(target_os = "macos")]
    fn load_keys_from_named_secure_storage(service_name: &str) -> ApiKeys {
        Self::deserialize_keys(
            match secure_storage::read_value_for_service(service_name, SECURE_STORAGE_KEY) {
                Ok(json) => json,
                Err(e) => {
                    if !matches!(e, secure_storage::Error::NotFound) {
                        log::error!("Failed to read API keys from secure storage: {e:#}");
                    }
                    return ApiKeys::default();
                }
            },
        )
    }

    fn deserialize_keys(key_json: String) -> ApiKeys {
        match serde_json::from_str(&key_json) {
            Ok(keys) => keys,
            Err(e) => {
                log::error!("Failed to deserialize API keys: {e:#}");
                ApiKeys::default()
            }
        }
    }

    fn apply_startup_loaded_keys(&mut self, keys: ApiKeys) -> bool {
        if self.startup_keys_mutated || self.keys == keys {
            return false;
        }
        self.keys = keys;
        true
    }

    fn write_keys_to_secure_storage(&mut self, ctx: &mut ModelContext<Self>) {
        let keys = self.keys.clone();

        let json = match serde_json::to_string(&keys) {
            Ok(json) => json,
            Err(e) => {
                log::error!("Failed to serialize API keys: {e:#}");
                return;
            }
        };

        if let Err(e) = ctx.secure_storage().write_value(SECURE_STORAGE_KEY, &json) {
            log::error!("Failed to write API keys to secure storage: {e:#}");
        }
    }
}

impl Entity for ApiKeyManager {
    type Event = ApiKeyManagerEvent;
}

impl SingletonEntity for ApiKeyManager {}

#[cfg(test)]
mod tests {
    use super::{ApiKeyManager, ApiKeys, AwsCredentialsState};

    fn manager_with(keys: ApiKeys) -> ApiKeyManager {
        ApiKeyManager {
            keys,
            aws_credentials_state: AwsCredentialsState::Missing,
            startup_keys_mutated: false,
        }
    }

    #[test]
    fn apply_startup_loaded_keys_hydrates_empty_manager() {
        let mut manager = manager_with(ApiKeys::default());
        let loaded = ApiKeys {
            openai: Some("secret".to_string()),
            ..ApiKeys::default()
        };

        assert!(manager.apply_startup_loaded_keys(loaded.clone()));
        assert_eq!(manager.keys(), &loaded);
    }

    #[test]
    fn apply_startup_loaded_keys_does_not_overwrite_local_edits() {
        let mut manager = manager_with(ApiKeys {
            openai: Some("user-value".to_string()),
            ..ApiKeys::default()
        });
        manager.startup_keys_mutated = true;

        assert!(!manager.apply_startup_loaded_keys(ApiKeys {
            openai: Some("old-startup-value".to_string()),
            ..ApiKeys::default()
        }));
        assert_eq!(manager.keys().openai.as_deref(), Some("user-value"));
    }
}
