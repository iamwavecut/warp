use super::*;

#[test]
fn custom_provider_compaction_fingerprint_tracks_protocol_and_cache_policy() {
    let initial = CustomProviderConfig {
        name: "local".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        models: vec!["model".to_string()],
        ..Default::default()
    };
    let fingerprint = |provider| configured_route_fingerprint("custom/local/model", &[provider]);
    let original = fingerprint(initial.clone()).unwrap();
    for api_type in [
        CustomApiType::OpenAiResponses,
        CustomApiType::AnthropicMessages,
    ] {
        let changed = CustomProviderConfig {
            api_type,
            ..initial.clone()
        };
        assert_ne!(fingerprint(changed).unwrap(), original);
    }
    let changed = CustomProviderConfig {
        prompt_caching: false,
        ..initial
    };
    assert_ne!(fingerprint(changed).unwrap(), original);
}
