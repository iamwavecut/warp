use super::*;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::settings::{AISettings, CustomProviderConfig};
use crate::test_util::{assert_eventually, settings::initialize_settings_for_tests};
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::LaunchMode;
use settings::Setting;
use warpui::{App, SingletonEntity};

#[test]
fn startup_loads_existing_custom_provider_models() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());
        app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .custom_providers
                .set_value(
                    vec![
                        CustomProviderConfig {
                            name: "local-keyless".to_string(),
                            base_url: "http://localhost:1234/v1".to_string(),
                            models: vec!["p0-keyless".to_string()],
                            api_key_env_var: None,
                            api_type: Default::default(),
                        },
                        CustomProviderConfig {
                            name: "local-keyed".to_string(),
                            base_url: "http://localhost:5678/v1".to_string(),
                            models: vec!["p0-keyed".to_string()],
                            api_key_env_var: Some("P0_LOCAL_API_KEY".to_string()),
                            api_type: Default::default(),
                        },
                    ],
                    ctx,
                )
                .unwrap();
        });

        let preferences = app.add_singleton_model(LLMPreferences::new);

        assert_eventually!(
            preferences.read(&app, |preferences, _| {
                preferences
                    .get_base_llm_choices_for_agent_mode()
                    .map(|model| model.id.as_str())
                    .collect::<Vec<_>>()
                    == vec!["custom/local-keyless/p0-keyless", "custom/local-keyed/p0-keyed"]
            }),
            "custom providers configured before startup should populate the Agent Mode catalog"
        );
    });
}

// -- DisableReason::should_clear_preference tests --

#[test]
fn should_clear_preference_admin_disabled() {
    // AdminDisabled always clears, regardless of BYOK status.
    assert!(DisableReason::AdminDisabled.should_clear_preference());
}

#[test]
fn should_clear_preference_unavailable() {
    assert!(DisableReason::Unavailable.should_clear_preference());
}

#[test]
fn should_not_clear_preference_out_of_requests() {
    // Transient — never clears.
    assert!(!DisableReason::OutOfRequests.should_clear_preference());
}

#[test]
fn should_not_clear_preference_provider_outage() {
    // Transient — never clears.
    assert!(!DisableReason::ProviderOutage.should_clear_preference());
}

#[test]
fn llm_info_deserializes_without_base_model_name() {
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": {
                "request_multiplier": 1,
                "credit_multiplier": null
            },
            "description": null,
            "disable_reason": null,
            "vision_supported": false,
            "spec": null,
            "provider": "Unknown"
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.base_model_name, "gpt-4o");
}

#[test]
fn llm_info_deserializes_host_configs_as_vec() {
    // Wire format from server: host_configs is a Vec
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": { "request_multiplier": 1, "credit_multiplier": null },
            "provider": "OpenAI",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" },
                { "enabled": false, "model_routing_host": "AwsBedrock" }
            ]
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize vec format");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.host_configs.len(), 2);
    assert!(
        info.host_configs
            .get(&LLMModelHost::DirectApi)
            .unwrap()
            .enabled
    );
    assert!(
        !info
            .host_configs
            .get(&LLMModelHost::AwsBedrock)
            .unwrap()
            .enabled
    );
}

#[test]
fn llm_info_round_trip_serializes_and_deserializes() {
    // Start with wire format (Vec)
    let wire_json = r#"{
            "display_name": "claude-3",
            "base_model_name": "claude-3",
            "id": "claude-3",
            "usage_metadata": { "request_multiplier": 2, "credit_multiplier": 1.5 },
            "description": "A powerful model",
            "vision_supported": true,
            "provider": "Anthropic",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" }
            ]
        }"#;

    // Deserialize from wire format
    let info: LLMInfo = serde_json::from_str(wire_json).expect("should deserialize");

    // Serialize (produces HashMap format)
    let serialized = serde_json::to_string(&info).expect("should serialize");

    // Deserialize again (from HashMap format)
    let round_tripped: LLMInfo =
        serde_json::from_str(&serialized).expect("should deserialize after round trip");

    assert_eq!(info, round_tripped);
}
