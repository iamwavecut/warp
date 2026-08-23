use super::{
    AgentAttributionToggleState, ProviderConnectionSignature, ProviderEditorErrorState,
    ProviderModelsValidationState, api_key_fingerprint, apply_provider_editor_persistence_result,
    custom_provider_api_key_update, custom_provider_editing_allowed,
    custom_provider_editor_actions_allowed, custom_provider_ids_are_persisted,
    custom_provider_key_is_still_referenced, derive_agent_attribution_toggle_state,
    find_live_provider_index, merge_provider_editor_config_with_live,
    normalize_custom_provider_ids, plan_custom_provider_key_migration,
    provider_connection_is_valid, provider_editor_duplicate_name_error,
    provider_editor_error_message, resolve_provider_connection,
};
use crate::settings::{
    CustomProviderCapabilities, CustomProviderConfig, CustomProviderConfigError,
    custom_provider_config_from_ui_with_capabilities,
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
    let (first_signature, _, first_api_key) = resolve_provider_connection(
        "http://localhost:1234/v1",
        " synthetic-credential-fixture ",
        "",
    )
    .unwrap();
    let (second_signature, _, second_api_key) = resolve_provider_connection(
        "http://localhost:1234/v1",
        "synthetic-credential-fixture",
        "",
    )
    .unwrap();

    assert_eq!(
        first_api_key.as_deref(),
        Some("synthetic-credential-fixture")
    );
    assert_eq!(
        second_api_key.as_deref(),
        Some("synthetic-credential-fixture")
    );
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
        transcription_model: None,
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
fn stale_provider_editor_does_not_reenable_tools_for_next_direct_route() {
    let initial = CustomProviderConfig {
        name: "local".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        models: vec!["model".to_string()],
        capabilities: CustomProviderCapabilities::default(),
        ..Default::default()
    };
    let after_capability_toggle = CustomProviderConfig {
        capabilities: CustomProviderCapabilities {
            tools: false,
            ..initial.capabilities.clone()
        },
        ..initial.clone()
    };
    let after_context_update = CustomProviderConfig {
        capabilities: CustomProviderCapabilities {
            context_window_tokens: Some(32_768),
            ..after_capability_toggle.capabilities.clone()
        },
        ..after_capability_toggle
    };
    let stale_editor = initial.clone();

    let merged =
        merge_provider_editor_config_with_live(stale_editor, &initial, Some(&after_context_update));

    assert!(!merged.capabilities.tools);
    assert_eq!(merged.capabilities.context_window_tokens, Some(32_768));

    let route = super::direct_openai::resolve_custom_provider_route(
        "custom/local/model",
        &[merged],
        &::ai::api_keys::ApiKeys::default(),
    )
    .expect("the direct route should use the live provider capabilities");
    assert!(!route.effective_capabilities().tools);
}

#[test]
fn edited_provider_field_is_not_replaced_by_live_snapshot() {
    let initial = CustomProviderConfig {
        name: "local".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        models: vec!["model".to_string()],
        ..Default::default()
    };
    let live = CustomProviderConfig {
        base_url: "http://localhost:5678/v1".to_string(),
        ..initial.clone()
    };
    let mut edited = initial.clone();
    edited.base_url = "http://localhost:9999/v1".to_string();

    let merged = merge_provider_editor_config_with_live(edited, &initial, Some(&live));

    assert_eq!(merged.base_url, "http://localhost:9999/v1");
}

#[test]
fn renamed_provider_editor_can_save_later_fields_without_reviving_stale_editors() {
    let live = vec![CustomProviderConfig {
        name: "renamed".to_string(),
        local_id: Some("provider-1".to_string()),
        ..Default::default()
    }];

    assert_eq!(find_live_provider_index(&live, "provider-1"), Some(0));
    assert_eq!(find_live_provider_index(&live, "stale-provider"), None);
}

#[test]
fn stale_editor_does_not_restore_secure_key_for_direct_route() {
    let editor_key = "synthetic-editor-credential";
    assert_eq!(
        custom_provider_api_key_update(editor_key, Some(api_key_fingerprint(editor_key))),
        None
    );
    assert_eq!(
        custom_provider_api_key_update("", Some(api_key_fingerprint(editor_key))),
        Some(None)
    );

    let provider = CustomProviderConfig {
        name: "local".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        models: vec!["model".to_string()],
        local_id: Some("provider-1".to_string()),
        ..Default::default()
    };
    let mut api_keys = ::ai::api_keys::ApiKeys::default();
    api_keys
        .custom
        .insert("local".to_string(), "synthetic-live-credential".to_string());

    let route = super::direct_openai::resolve_custom_provider_route(
        "custom/local/model",
        &[provider],
        &api_keys,
    )
    .expect("the direct route should use the live secure key");
    assert_eq!(route.api_key.as_deref(), Some("synthetic-live-credential"));
}

#[test]
fn provider_identity_rejects_old_name_reuse_after_delete() {
    let live = vec![CustomProviderConfig {
        name: "local".to_string(),
        local_id: Some("replacement-provider".to_string()),
        ..Default::default()
    }];

    assert_eq!(find_live_provider_index(&live, "stale-provider"), None);
}

#[test]
fn missing_or_duplicate_provider_ids_are_repaired() {
    let mut providers = vec![
        CustomProviderConfig {
            name: "first".to_string(),
            local_id: None,
            ..Default::default()
        },
        CustomProviderConfig {
            name: "second".to_string(),
            local_id: Some("duplicate".to_string()),
            ..Default::default()
        },
        CustomProviderConfig {
            name: "third".to_string(),
            local_id: Some("duplicate".to_string()),
            ..Default::default()
        },
    ];

    assert!(normalize_custom_provider_ids(&mut providers));
    let ids = providers
        .iter()
        .map(|provider| {
            provider
                .local_id
                .as_deref()
                .expect("provider ID")
                .to_string()
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), providers.len());
}

#[test]
fn custom_provider_editor_waits_for_secure_key_hydration() {
    assert!(!custom_provider_editing_allowed(false));
    assert!(custom_provider_editing_allowed(true));
}

#[test]
fn provider_editor_actions_require_persisted_provider_identity() {
    let persisted = vec![CustomProviderConfig {
        local_id: Some("stable".to_string()),
        name: "local".to_string(),
        ..Default::default()
    }];
    let ephemeral = vec![CustomProviderConfig {
        local_id: None,
        name: "local".to_string(),
        ..Default::default()
    }];

    assert!(custom_provider_ids_are_persisted(&persisted));
    assert!(!custom_provider_ids_are_persisted(&ephemeral));
    assert!(!custom_provider_editor_actions_allowed(false, true));
    assert!(!custom_provider_editor_actions_allowed(true, false));
    assert!(custom_provider_editor_actions_allowed(true, true));
}

#[test]
fn duplicate_provider_key_survives_rename_until_last_legacy_name_is_removed() {
    let providers_after_rename = vec![
        CustomProviderConfig {
            local_id: Some("renamed-provider".to_string()),
            name: "renamed".to_string(),
            ..Default::default()
        },
        CustomProviderConfig {
            local_id: Some("legacy-provider".to_string()),
            name: "legacy".to_string(),
            ..Default::default()
        },
    ];
    let existing_keys = std::collections::HashMap::from([(
        "legacy".to_string(),
        "synthetic-credential-fixture".to_string(),
    )]);
    let migration = plan_custom_provider_key_migration(
        &providers_after_rename,
        "renamed-provider",
        "legacy",
        "renamed",
        true,
        None,
        &existing_keys,
    )
    .expect("rename should copy the shared legacy key");

    assert!(!migration.remove_original);
    assert_eq!(
        migration.current_key.as_deref(),
        Some("synthetic-credential-fixture")
    );
    assert!(custom_provider_key_is_still_referenced(
        &providers_after_rename,
        "legacy"
    ));

    let providers_after_delete = vec![providers_after_rename[0].clone()];
    assert!(!custom_provider_key_is_still_referenced(
        &providers_after_delete,
        "legacy"
    ));
}

#[test]
fn provider_editor_error_state_is_visible_and_clears_after_success() {
    let state = ProviderEditorErrorState::default();
    state.set(
        "provider-a",
        provider_editor_error_message(
            "provider-a",
            CustomProviderConfigError::MissingTranscriptionModel,
        ),
    );

    let visible = state
        .message("provider-a")
        .expect("validation error should be visible for its provider");
    assert!(visible.contains("transcription"));
    assert!(visible.contains("Fix the provider settings"));

    let targets = vec![(
        "provider-a".to_string(),
        "provider-a".to_string(),
        state.clone(),
    )];
    assert!(apply_provider_editor_persistence_result(Ok(()), &targets).is_ok());
    assert_eq!(state.message("provider-a"), None);
}

#[test]
fn provider_editor_persistence_failure_keeps_provider_error_visible() {
    let state = ProviderEditorErrorState::default();
    state.set("provider-a", "stale provider error");
    let targets = vec![(
        "provider-a".to_string(),
        "provider-a".to_string(),
        state.clone(),
    )];

    let result = apply_provider_editor_persistence_result::<()>(
        Err(anyhow::anyhow!("synthetic local settings write failure")),
        &targets,
    );

    assert!(result.is_err());
    let visible = state
        .message("provider-a")
        .expect("persistence failure should remain visible for its provider");
    assert!(visible.contains("could not be saved"));
    assert!(visible.contains("Fix the provider settings"));

    state.set_global(visible.clone());
    assert_eq!(state.global_message(), Some(visible));
    state.clear_global();
    assert_eq!(state.global_message(), None);
}

#[test]
fn provider_editor_error_message_preserves_duplicate_and_route_reasons() {
    let duplicate = provider_editor_duplicate_name_error("provider-a");
    assert!(duplicate.contains("already exists"));
    assert!(duplicate.contains("unique"));

    let route = provider_editor_error_message(
        "provider-a",
        "selected chat model `chat-a` is not listed for the provider route",
    );
    assert!(route.contains("selected chat model"));
    assert!(route.contains("provider route"));
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
