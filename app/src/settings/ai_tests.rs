use super::*;
use crate::auth::AuthStateProvider;
use crate::test_util::settings::initialize_settings_for_tests;
use warpui::{App, SingletonEntity};

#[test]
fn parses_custom_provider_model_input_for_ui() {
    assert_eq!(
        parse_custom_provider_models("qwen3-coder, llama-local\nqwen3-coder\n  gpt-oss "),
        vec!["qwen3-coder", "llama-local", "gpt-oss"]
    );
}

#[test]
fn normalizes_custom_provider_env_var_from_ui() {
    assert_eq!(
        normalize_custom_provider_env_var("  $LOCAL_OPENAI_API_KEY "),
        Some("LOCAL_OPENAI_API_KEY".to_string())
    );
    assert_eq!(normalize_custom_provider_env_var("   "), None);
}

#[test]
fn custom_provider_names_are_unique_only_when_unambiguous() {
    let providers = vec![
        CustomProviderConfig {
            name: "duplicate".to_string(),
            ..Default::default()
        },
        CustomProviderConfig {
            name: "duplicate".to_string(),
            ..Default::default()
        },
        CustomProviderConfig {
            name: "unique".to_string(),
            ..Default::default()
        },
    ];

    assert!(!custom_provider_name_is_unique("duplicate", &providers));
    assert!(custom_provider_name_is_unique("unique", &providers));
    assert!(!custom_provider_name_is_unique("missing", &providers));
}

#[test]
fn builds_openai_compatible_custom_provider_from_ui_fields() {
    let provider = custom_provider_config_from_ui(
        " local-openai-compatible ",
        " http://localhost:1234/v1/ ",
        "qwen3-coder\nllama-local",
        "$LOCAL_OPENAI_API_KEY",
    )
    .expect("provider config should be valid");

    assert_eq!(
        provider,
        CustomProviderConfig {
            local_id: None,
            name: "local-openai-compatible".to_string(),
            base_url: "http://localhost:1234/v1/".to_string(),
            models: vec!["qwen3-coder".to_string(), "llama-local".to_string()],
            api_key_env_var: Some("LOCAL_OPENAI_API_KEY".to_string()),
            api_type: CustomApiType::OpenAiCompatible,
            capabilities: CustomProviderCapabilities::default(),
        }
    );
}

#[test]
fn legacy_custom_provider_config_uses_compatible_capability_defaults() {
    let provider: CustomProviderConfig = serde_json::from_value(serde_json::json!({
        "name": "legacy",
        "base_url": "http://localhost:1234/v1",
        "models": ["local-model"],
        "api_type": "open_ai_compatible"
    }))
    .expect("legacy provider config should remain valid");

    assert_eq!(provider.capabilities, CustomProviderCapabilities::default());
    assert!(provider.capabilities.chat);
    assert!(provider.capabilities.tools);
    assert!(!provider.capabilities.vision);
    assert!(!provider.capabilities.embeddings);
    assert!(!provider.capabilities.transcription);
    assert!(provider.validate().is_ok());

    let partial_capabilities: CustomProviderCapabilities =
        serde_json::from_value(serde_json::json!({"vision": true}))
            .expect("missing chat/tool fields should use compatibility defaults");
    assert!(partial_capabilities.chat);
    assert!(partial_capabilities.tools);
    assert!(partial_capabilities.vision);
}

#[test]
fn custom_provider_capabilities_round_trip_explicit_values() {
    let capabilities = CustomProviderCapabilities {
        chat: false,
        tools: false,
        vision: true,
        embeddings: true,
        embedding_model: Some("local-embedding".to_string()),
        transcription: true,
        transcription_model: Some("local-whisper".to_string()),
        context_window_tokens: Some(32_000),
    };
    let provider = CustomProviderConfig {
        local_id: None,
        name: "local".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        models: vec!["model".to_string()],
        api_key_env_var: None,
        api_type: CustomApiType::OpenAiCompatible,
        capabilities: capabilities.clone(),
    };

    let encoded = serde_json::to_value(&provider).expect("provider should serialize");
    let decoded: CustomProviderConfig =
        serde_json::from_value(encoded).expect("provider should deserialize");

    assert_eq!(decoded.capabilities, capabilities);
    assert!(decoded.validate().is_ok());
}

#[test]
fn custom_provider_context_window_validation_rejects_zero_and_tiny_values() {
    for tokens in [0, 1, CUSTOM_PROVIDER_MIN_CONTEXT_WINDOW_TOKENS - 1] {
        let provider = CustomProviderConfig {
            capabilities: CustomProviderCapabilities {
                context_window_tokens: Some(tokens),
                ..Default::default()
            },
            ..CustomProviderConfig::default()
        };

        let error = provider
            .validate()
            .expect_err("invalid context window should return a local configuration error");
        assert!(error.to_string().contains("context window"));
    }

    let provider = CustomProviderConfig {
        capabilities: CustomProviderCapabilities {
            context_window_tokens: Some(CUSTOM_PROVIDER_MIN_CONTEXT_WINDOW_TOKENS),
            ..Default::default()
        },
        ..CustomProviderConfig::default()
    };
    assert!(provider.validate().is_ok());
}

#[test]
fn custom_provider_transcription_requires_an_explicit_model() {
    let provider = CustomProviderConfig {
        capabilities: CustomProviderCapabilities {
            transcription: true,
            ..Default::default()
        },
        ..Default::default()
    };

    let error = provider
        .validate()
        .expect_err("transcription without a model must be rejected locally");
    assert_eq!(error, CustomProviderConfigError::MissingTranscriptionModel);
    assert!(error.to_string().contains("transcription model"));

    let capabilities = custom_provider_capabilities_from_ui_with_transcription_model(
        true,
        true,
        false,
        false,
        true,
        Some(" local-whisper "),
        "",
    )
    .expect("a non-empty transcription model should be accepted");
    assert_eq!(
        capabilities.transcription_model.as_deref(),
        Some("local-whisper")
    );
}

#[test]
fn custom_provider_embeddings_require_an_explicit_model() {
    let provider = CustomProviderConfig {
        capabilities: CustomProviderCapabilities {
            embeddings: true,
            ..Default::default()
        },
        ..Default::default()
    };

    assert!(
        provider.validate().is_ok(),
        "legacy intent must not disable chat"
    );
    assert!(
        !crate::ai::agent::api::direct_openai::effective_capabilities_for_config(
            &provider.capabilities
        )
        .embeddings
    );
    assert_eq!(
        custom_provider_capabilities_from_ui_with_models(
            true, true, false, true, false, None, None, "",
        ),
        Err(CustomProviderConfigError::MissingEmbeddingModel)
    );
    let capabilities = custom_provider_capabilities_from_ui_with_models(
        true,
        true,
        false,
        true,
        false,
        Some(" local-embedding "),
        None,
        "",
    )
    .expect("a non-empty embedding model should be accepted");
    assert_eq!(
        capabilities.embedding_model.as_deref(),
        Some("local-embedding")
    );
}

#[test]
fn custom_provider_transcription_model_round_trips_and_legacy_config_defaults_to_none() {
    let provider = CustomProviderConfig {
        capabilities: CustomProviderCapabilities {
            transcription: true,
            transcription_model: Some("local-whisper".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let encoded = serde_json::to_value(&provider).expect("provider should serialize");
    let decoded: CustomProviderConfig =
        serde_json::from_value(encoded).expect("provider should deserialize");
    assert_eq!(
        decoded.capabilities.transcription_model.as_deref(),
        Some("local-whisper")
    );

    let legacy: CustomProviderConfig = serde_json::from_value(serde_json::json!({
        "name": "legacy",
        "base_url": "http://localhost:1234/v1",
        "models": ["local-model"]
    }))
    .expect("legacy provider config should remain valid");
    assert_eq!(legacy.capabilities.embedding_model, None);
    assert_eq!(legacy.capabilities.transcription_model, None);
}

#[test]
fn local_transcription_configuration_persists_without_credentials() {
    let provider = CustomProviderConfig {
        name: "local-transcription".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        models: vec!["chat-model".to_string()],
        api_key_env_var: None,
        capabilities: CustomProviderCapabilities {
            transcription: true,
            transcription_model: Some("whisper-local".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let encoded = serde_json::to_string(&provider).expect("local config should serialize");
    assert!(!encoded.contains("credential"));
    let restored: CustomProviderConfig =
        serde_json::from_str(&encoded).expect("local config should deserialize");
    assert_eq!(restored, provider);
}

#[test]
fn custom_provider_capabilities_from_ui_rejects_invalid_context_window() {
    let error = custom_provider_capabilities_from_ui(true, true, false, false, false, "1")
        .expect_err("tiny context window should be rejected before saving settings");
    assert!(error.to_string().contains("context window"));

    let capabilities = custom_provider_capabilities_from_ui_with_models(
        true,
        false,
        true,
        true,
        true,
        Some("local-embedding"),
        Some("local-whisper"),
        "",
    )
    .expect("empty context window means use the adapter fallback");
    assert!(capabilities.chat);
    assert!(!capabilities.tools);
    assert!(capabilities.vision);
    assert!(capabilities.embeddings);
    assert!(capabilities.transcription);
    assert_eq!(capabilities.context_window_tokens, None);
}

#[test]
fn ranks_custom_provider_model_suggestions_with_prefix_priority() {
    let suggestions = ranked_custom_provider_model_suggestions(
        "qwen code",
        &[
            "llama-local".to_string(),
            "qwen3-coder".to_string(),
            "coder-qwen".to_string(),
            "qwen2.5-coder".to_string(),
        ],
        &[],
    );

    assert_eq!(
        suggestions,
        vec![
            "qwen2.5-coder".to_string(),
            "qwen3-coder".to_string(),
            "coder-qwen".to_string()
        ]
    );
}

#[test]
fn ranked_custom_provider_model_suggestions_excludes_selected_models() {
    let suggestions = ranked_custom_provider_model_suggestions(
        "qwen",
        &[
            "qwen3-coder".to_string(),
            "qwen2.5-coder".to_string(),
            "qwen3-coder".to_string(),
        ],
        &["qwen3-coder".to_string()],
    );

    assert_eq!(suggestions, vec!["qwen2.5-coder".to_string()]);
}

fn add_ai_enablement_dependencies_for_test(app: &mut App) {
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(UserWorkspaces::default_mock);
}

// FocusedTerminalInfo Tests

#[test]
fn test_update_both_values_changed() {
    App::test((), |mut app| async move {
        // Create FocusedTerminalInfo with default values (false, false)
        let model_handle = app.add_model(|_| FocusedTerminalInfo::default());

        // Setup event tracking
        let (sender, receiver) = async_channel::unbounded();
        app.update(|ctx| {
            let sender = sender.clone();
            ctx.subscribe_to_model(
                &model_handle,
                move |_, event: &FocusedTerminalInfoEvent, _| match event {
                    FocusedTerminalInfoEvent::TerminalInfoUpdated => {
                        let _ = sender.try_send(());
                    }
                },
            );
        });

        // Update both values to (true, false)
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, false, ctx);
        });

        // Verify model state
        model_handle.read(&app, |model, _| {
            assert!(model.contains_any_remote_blocks());
            assert!(!model.contains_any_restored_remote_blocks());
        });

        // Verify event was emitted exactly once
        let mut count = 0;
        while receiver.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 1);
    });
}

#[test]
fn test_update_additional_value_changed() {
    App::test((), |mut app| async move {
        // Create FocusedTerminalInfo with default values (false, false)
        let model_handle = app.add_model(|_| FocusedTerminalInfo::default());

        // Setup event tracking
        let (sender, receiver) = async_channel::unbounded();
        app.update(|ctx| {
            let sender = sender.clone();
            ctx.subscribe_to_model(
                &model_handle,
                move |_, event: &FocusedTerminalInfoEvent, _| match event {
                    FocusedTerminalInfoEvent::TerminalInfoUpdated => {
                        let _ = sender.try_send(());
                    }
                },
            );
        });

        // First update to (true, false)
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, false, ctx);
        });

        // Clear events by draining the channel
        while receiver.try_recv().is_ok() {}

        // Now update to (true, true) - only changing restored blocks
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, true, ctx);
        });

        // Verify model state
        model_handle.read(&app, |model, _| {
            assert!(model.contains_any_remote_blocks());
            assert!(model.contains_any_restored_remote_blocks());
        });

        // Verify event was emitted exactly once
        let mut count = 0;
        while receiver.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 1);
    });
}

#[test]
fn test_update_no_change() {
    App::test((), |mut app| async move {
        // Create FocusedTerminalInfo with default values (false, false)
        let model_handle = app.add_model(|_| FocusedTerminalInfo::default());

        // Setup event tracking
        let (sender, receiver) = async_channel::unbounded();
        app.update(|ctx| {
            let sender = sender.clone();
            ctx.subscribe_to_model(
                &model_handle,
                move |_, event: &FocusedTerminalInfoEvent, _| match event {
                    FocusedTerminalInfoEvent::TerminalInfoUpdated => {
                        let _ = sender.try_send(());
                    }
                },
            );
        });

        // First update to (true, true)
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, true, ctx);
        });

        // Clear events by draining the channel
        while receiver.try_recv().is_ok() {}

        // Update with same values (true, true)
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, true, ctx);
        });

        // Verify model state remains the same
        model_handle.read(&app, |model, _| {
            assert!(model.contains_any_remote_blocks());
            assert!(model.contains_any_restored_remote_blocks());
        });

        // Verify no event was emitted
        let mut count = 0;
        while receiver.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 0);
    });
}

#[test]
fn test_update_only_remote_toggles() {
    App::test((), |mut app| async move {
        // Create FocusedTerminalInfo with default values (false, false)
        let model_handle = app.add_model(|_| FocusedTerminalInfo::default());

        // Setup event tracking
        let (sender, receiver) = async_channel::unbounded();
        app.update(|ctx| {
            let sender = sender.clone();
            ctx.subscribe_to_model(
                &model_handle,
                move |_, event: &FocusedTerminalInfoEvent, _| match event {
                    FocusedTerminalInfoEvent::TerminalInfoUpdated => {
                        let _ = sender.try_send(());
                    }
                },
            );
        });

        // First update to (true, true)
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, true, ctx);
        });

        // Clear events by draining the channel
        while receiver.try_recv().is_ok() {}

        // Update with (false, true) - only remote blocks changes
        model_handle.update(&mut app, |model, ctx| {
            model.update(false, true, ctx);
        });

        // Verify model state
        model_handle.read(&app, |model, _| {
            assert!(!model.contains_any_remote_blocks());
            assert!(model.contains_any_restored_remote_blocks());
        });

        // Verify event was emitted exactly once
        let mut count = 0;
        while receiver.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 1);
    });
}

#[test]
fn test_update_only_restored_toggles() {
    App::test((), |mut app| async move {
        // Create FocusedTerminalInfo with default values (false, false)
        let model_handle = app.add_model(|_| FocusedTerminalInfo::default());

        // Setup event tracking
        let (sender, receiver) = async_channel::unbounded();
        app.update(|ctx| {
            let sender = sender.clone();
            ctx.subscribe_to_model(
                &model_handle,
                move |_, event: &FocusedTerminalInfoEvent, _| match event {
                    FocusedTerminalInfoEvent::TerminalInfoUpdated => {
                        let _ = sender.try_send(());
                    }
                },
            );
        });

        // First update to (true, true)
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, true, ctx);
        });

        // Clear events by draining the channel
        while receiver.try_recv().is_ok() {}

        // Update with (true, false) - only restored blocks changes
        model_handle.update(&mut app, |model, ctx| {
            model.update(true, false, ctx);
        });

        // Verify model state
        model_handle.read(&app, |model, _| {
            assert!(model.contains_any_remote_blocks());
            assert!(!model.contains_any_restored_remote_blocks());
        });

        // Verify event was emitted exactly once
        let mut count = 0;
        while receiver.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 1);
    });
}

// ToolbarCommandMap Tests

#[test]
fn test_toolbar_command_map_deserialize_from_map() {
    let json = serde_json::json!({
        "^claude": "Claude",
        "^gemini": "Gemini",
        "^codex": ""
    });
    let map: ToolbarCommandMap = serde_json::from_value(json).unwrap();
    assert_eq!(map.0.len(), 3);
    assert_eq!(map.0["^claude"], "Claude");
    assert_eq!(map.0["^gemini"], "Gemini");
    assert_eq!(map.0["^codex"], "");
}

#[test]
fn test_toolbar_command_map_deserialize_from_legacy_vec() {
    let json = serde_json::json!(["^claude", "^gemini", "^custom"]);
    let map: ToolbarCommandMap = serde_json::from_value(json).unwrap();
    assert_eq!(map.0.len(), 3);
    // Legacy vec format should assign empty agent values.
    for (_, agent) in map.0.iter() {
        assert_eq!(agent, "");
    }
    let keys: Vec<_> = map.0.keys().collect();
    assert_eq!(keys, vec!["^claude", "^gemini", "^custom"]);
}

#[test]
fn test_toolbar_command_map_from_file_value_map_format() {
    use settings_value::SettingsValue;

    let value = serde_json::json!({
        "^claude": "Claude",
        "^amp": "Amp"
    });
    let map = ToolbarCommandMap::from_file_value(&value).unwrap();
    assert_eq!(map.0.len(), 2);
    assert_eq!(map.0["^claude"], "Claude");
    assert_eq!(map.0["^amp"], "Amp");
}

#[test]
fn test_toolbar_command_map_from_file_value_legacy_array() {
    use settings_value::SettingsValue;

    // Patterns are intentionally non-alphabetical to verify insertion order is preserved.
    let value = serde_json::json!(["^zebra", "^alpha", "^middle"]);
    let map = ToolbarCommandMap::from_file_value(&value).unwrap();
    assert_eq!(map.0.len(), 3);
    assert_eq!(map.0["^zebra"], "");
    assert_eq!(map.0["^alpha"], "");
    assert_eq!(map.0["^middle"], "");
    let keys: Vec<_> = map.0.keys().collect();
    assert_eq!(keys, vec!["^zebra", "^alpha", "^middle"]);
}

#[test]
fn test_toolbar_command_map_from_file_value_invalid() {
    use settings_value::SettingsValue;

    let value = serde_json::json!(42);
    assert!(ToolbarCommandMap::from_file_value(&value).is_none());
}

#[test]
fn test_toolbar_command_map_roundtrip() {
    use settings_value::SettingsValue;

    let mut inner = IndexMap::new();
    inner.insert("^claude".to_string(), "Claude".to_string());
    inner.insert("^custom".to_string(), String::new());
    let original = ToolbarCommandMap::new(inner);

    let file_value = original.to_file_value();
    let restored = ToolbarCommandMap::from_file_value(&file_value).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn test_toolbar_command_map_matched_agent() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);

        let mut map = IndexMap::new();
        map.insert("^claude".to_string(), "Claude".to_string());
        map.insert("^gemini".to_string(), "Gemini".to_string());
        map.insert("^custom-tool".to_string(), String::new());

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            report_if_error!(
                settings
                    .cli_agent_footer_enabled_commands
                    .set_value(ToolbarCommandMap::new(map), ctx)
            );
        });

        app.read(|ctx| {
            let agent = CompiledCommandsForCodingAgentToolbar::matched_agent(ctx, "claude chat");
            assert_eq!(agent, Some(CLIAgent::Claude));

            let agent = CompiledCommandsForCodingAgentToolbar::matched_agent(ctx, "gemini ask");
            assert_eq!(agent, Some(CLIAgent::Gemini));

            let agent =
                CompiledCommandsForCodingAgentToolbar::matched_agent(ctx, "custom-tool --flag");
            assert_eq!(agent, Some(CLIAgent::Unknown));

            let agent =
                CompiledCommandsForCodingAgentToolbar::matched_agent(ctx, "unmatched-command");
            assert_eq!(agent, None);
        });
    });
}
