use super::{
    AgentAttributionToggleState, ProviderConnectionSignature, derive_agent_attribution_toggle_state,
    resolve_provider_connection,
};
use crate::workspaces::workspace::AdminEnablementSetting;

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
fn provider_connection_with_unset_environment_variable_is_an_error() {
    let error = resolve_provider_connection(
        "http://localhost:1234/v1",
        "",
        "WARP_OSS_TEST_UNSET_PROVIDER_API_KEY",
    )
    .unwrap_err();

    assert_eq!(
        error,
        "Environment variable WARP_OSS_TEST_UNSET_PROVIDER_API_KEY is not set."
    );
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
