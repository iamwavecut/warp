use super::{
    AgentAttributionToggleState, ProviderConnectionSignature, ProviderModelsValidationState,
    derive_agent_attribution_toggle_state, provider_connection_is_valid,
    resolve_provider_connection,
};
use crate::settings::{
    CustomProviderCapabilities, custom_provider_config_from_ui_with_capabilities,
};
use crate::workspaces::workspace::AdminEnablementSetting;
use std::ffi::OsString;

struct EnvVarGuard {
    name: String,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(name: String, value: &str) -> Self {
        let previous = std::env::var_os(&name);
        unsafe { std::env::set_var(&name, value) };
        Self { name, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(&self.name, value) },
            None => unsafe { std::env::remove_var(&self.name) },
        }
    }
}

#[test]
fn provider_connection_without_key_is_valid_and_unsigned() {
    let (signature, base_url, api_key) =
        resolve_provider_connection(" http://localhost:1234/v1/ ", "", "").unwrap();

    assert_eq!(base_url, "http://localhost:1234/v1");
    assert_eq!(api_key, None);
    assert_eq!(
        signature,
        ProviderConnectionSignature {
            base_url: "http://localhost:1234/v1".to_string(),
            api_key_fingerprint: None,
        }
    );
}

#[test]
fn keyless_provider_validation_enables_model_selection() {
    let (signature, _, api_key) =
        resolve_provider_connection("http://localhost:1234/v1", "", "").unwrap();
    let validation_state = ProviderModelsValidationState::Valid(signature.clone());

    assert_eq!(api_key, None);
    assert!(provider_connection_is_valid(&validation_state, &signature));
}

#[test]
fn provider_connection_with_direct_key_has_stable_signature() {
    let (first_signature, _, first_api_key) =
        resolve_provider_connection("http://localhost:1234/v1", " direct-key ", "").unwrap();
    let (second_signature, _, second_api_key) =
        resolve_provider_connection("http://localhost:1234/v1", "direct-key", "").unwrap();

    assert_eq!(first_api_key.as_deref(), Some("direct-key"));
    assert_eq!(second_api_key.as_deref(), Some("direct-key"));
    assert_eq!(first_signature, second_signature);
    assert!(first_signature.api_key_fingerprint.is_some());
}

#[test]
#[serial_test::serial]
fn provider_connection_with_unset_environment_variable_is_an_error() {
    let env_var = format!(
        "WARP_OSS_TEST_UNSET_PROVIDER_API_KEY_{}",
        uuid::Uuid::new_v4().simple()
    );
    let error = resolve_provider_connection("http://localhost:1234/v1", "", &env_var).unwrap_err();

    assert_eq!(error, format!("Environment variable {env_var} is not set."));
}

#[test]
#[serial_test::serial]
fn provider_connection_with_empty_environment_variable_is_an_error() {
    let env_var = format!(
        "WARP_OSS_TEST_EMPTY_PROVIDER_API_KEY_{}",
        uuid::Uuid::new_v4().simple()
    );
    let _env_var_guard = EnvVarGuard::set(env_var.clone(), "");

    let error = resolve_provider_connection("http://localhost:1234/v1", "", &format!("${env_var}"))
        .unwrap_err();

    assert_eq!(error, "API key is empty.");
}

#[test]
fn provider_connection_with_empty_or_invalid_base_url_is_an_error() {
    assert_eq!(
        resolve_provider_connection("", "", "").unwrap_err(),
        "Enter a base URL first."
    );
    assert_eq!(
        resolve_provider_connection("not-a-url", "", "").unwrap_err(),
        "Base URL is not a valid URL."
    );
}

#[test]
fn provider_editor_save_preserves_capabilities_when_provider_is_renamed() {
    let capabilities = CustomProviderCapabilities {
        chat: true,
        tools: false,
        vision: true,
        embeddings: true,
        transcription: false,
        context_window_tokens: Some(32_000),
    };

    let config = custom_provider_config_from_ui_with_capabilities(
        "renamed-local",
        "http://localhost:1234/v1",
        "model-a\nmodel-b",
        "$LOCAL_OPENAI_API_KEY",
        capabilities.clone(),
    )
    .unwrap()
    .expect("complete provider editor values should produce a config");

    assert_eq!(config.name, "renamed-local");
    assert_eq!(config.capabilities, capabilities);
}

#[test]
fn respect_user_setting_returns_user_pref_unlocked() {
    let state = derive_agent_attribution_toggle_state(
        &AdminEnablementSetting::RespectUserSetting,
        true,
        true,
    );
    assert_eq!(
        state,
        AgentAttributionToggleState {
            is_enabled: true,
            is_forced_by_org: false,
            is_disabled: false,
        }
    );
}

#[test]
fn respect_user_setting_with_user_off_returns_unchecked_unlocked() {
    let state = derive_agent_attribution_toggle_state(
        &AdminEnablementSetting::RespectUserSetting,
        false,
        true,
    );
    assert_eq!(
        state,
        AgentAttributionToggleState {
            is_enabled: false,
            is_forced_by_org: false,
            is_disabled: false,
        }
    );
}

#[test]
fn team_enable_locks_toggle_on_regardless_of_user_pref() {
    let state = derive_agent_attribution_toggle_state(&AdminEnablementSetting::Enable, false, true);
    assert_eq!(
        state,
        AgentAttributionToggleState {
            is_enabled: true,
            is_forced_by_org: true,
            is_disabled: true,
        }
    );
}

#[test]
fn team_disable_locks_toggle_off_regardless_of_user_pref() {
    let state = derive_agent_attribution_toggle_state(&AdminEnablementSetting::Disable, true, true);
    assert_eq!(
        state,
        AgentAttributionToggleState {
            is_enabled: false,
            is_forced_by_org: true,
            is_disabled: true,
        }
    );
}

#[test]
fn ai_globally_disabled_marks_toggle_disabled_but_not_forced() {
    let state = derive_agent_attribution_toggle_state(
        &AdminEnablementSetting::RespectUserSetting,
        true,
        false,
    );
    assert_eq!(
        state,
        AgentAttributionToggleState {
            is_enabled: true,
            is_forced_by_org: false,
            is_disabled: true,
        }
    );
}

#[test]
fn team_force_takes_precedence_over_global_ai_disabled() {
    let state =
        derive_agent_attribution_toggle_state(&AdminEnablementSetting::Enable, false, false);
    assert_eq!(
        state,
        AgentAttributionToggleState {
            is_enabled: true,
            is_forced_by_org: true,
            is_disabled: true,
        }
    );
}
