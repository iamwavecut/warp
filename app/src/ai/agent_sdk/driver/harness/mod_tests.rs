use warp_cli::agent::Harness;

use super::{auth_check_command_for, validate_cli_installed};
use crate::ai::agent_sdk::driver::AgentDriverError;

#[test]
fn claude_model_environment_supports_custom_acp_models_without_hosted_metadata() {
    use crate::ai::ambient_agents::task::HarnessModelConfig;
    for model in ["custom-model", "default", ""] {
        let config: HarnessModelConfig =
            serde_json::from_value(serde_json::json!({"model_id": model})).unwrap();
        let env = super::harness_model_env_vars(Harness::Claude, Some(&config));
        assert_eq!(
            env.get(std::ffi::OsStr::new("ANTHROPIC_MODEL"))
                .map(|s| s.to_str().unwrap()),
            (!model.is_empty()).then_some(model)
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("ANTHROPIC_CUSTOM_MODEL_OPTION"))
                .map(|s| s.to_str().unwrap()),
            (!model.is_empty() && model != "default").then_some(model)
        );
        assert!(super::harness_model_env_vars(Harness::Codex, Some(&config)).is_empty());
    }
    assert!(super::harness_model_env_vars(Harness::Claude, None).is_empty());
}

#[test]
fn unsupported_hosted_harnesses_cannot_use_acp_manifest_dispatch() {
    for harness in [Harness::Oz, Harness::OpenCode, Harness::Unknown] {
        assert!(
            super::harness_kind_with_transport(harness, warp_cli::agent::HarnessTransport::Acp)
                .is_err()
        );
    }
}

#[test]
fn pi_uses_local_acp_without_a_hosted_or_pty_fallback() {
    use super::{HarnessKind, harness_kind, harness_kind_with_transport};
    use warp_cli::agent::HarnessTransport;
    let launch = super::acp::AcpLaunchSpec::for_harness(Harness::Pi).unwrap();
    assert_eq!(launch.program, "pi-acp");
    assert!(launch.args.is_empty());
    let HarnessKind::ThirdParty(default_runner) = harness_kind(Harness::Pi).unwrap() else {
        panic!("Pi must use the local ACP adapter by default");
    };
    assert!(!default_runner.drives_cli_agent_session());
    assert!(matches!(
        harness_kind_with_transport(Harness::Pi, HarnessTransport::Acp).unwrap(),
        HarnessKind::ThirdParty(_)
    ));
    assert!(auth_check_command_for(Harness::Pi).is_none());
}

fn assert_harness_setup_failed(err: &AgentDriverError) -> (&str, &str) {
    match err {
        AgentDriverError::HarnessSetupFailed { harness, reason } => (harness, reason),
        other => panic!("expected HarnessSetupFailed, got: {other}"),
    }
}

#[cfg(not(windows))]
#[test]
fn validate_cli_installed_succeeds_for_known_binary() {
    assert!(validate_cli_installed("ls", None).is_ok());
}

#[test]
fn validate_cli_installed_fails_for_missing_binary() {
    let err = validate_cli_installed("__nonexistent_cli_abc123__", None).unwrap_err();
    let (harness, reason) = assert_harness_setup_failed(&err);
    assert_eq!(harness, "__nonexistent_cli_abc123__");
    assert!(reason.contains("not found"));
    assert!(!reason.contains("Install it first"));
}

#[test]
fn validate_cli_installed_includes_docs_url_in_error() {
    let url = "https://example.com/install";
    let err = validate_cli_installed("__nonexistent_cli_abc123__", Some(url)).unwrap_err();
    let (_, reason) = assert_harness_setup_failed(&err);
    assert!(reason.contains(url));
    assert!(reason.contains("Install it first"));
}

// --- Runtime error pattern tests ---

#[test]
fn claude_runtime_error_patterns_returns_slice() {
    use super::ThirdPartyHarness;
    use super::claude_code::ClaudeHarness;
    // Patterns are initially empty until validated needles are filled in.
    // The trait method must still be callable.
    let _: &[&str] = ClaudeHarness.runtime_error_patterns();
}

#[test]
fn codex_runtime_error_patterns_returns_slice() {
    use super::ThirdPartyHarness;
    use super::codex::CodexHarness;
    let _: &[&str] = CodexHarness.runtime_error_patterns();
}

#[test]
fn gemini_runtime_error_patterns_is_empty_by_default() {
    use super::ThirdPartyHarness;
    use super::gemini::GeminiHarness;
    assert!(GeminiHarness.runtime_error_patterns().is_empty());
}

#[test]
fn auth_check_command_for_gemini_is_none() {
    assert!(auth_check_command_for(Harness::Gemini).is_none());
}

#[test]
fn auth_check_command_for_oz_is_none() {
    assert!(auth_check_command_for(Harness::Oz).is_none());
}

#[test]
fn auth_check_command_for_unsupported_is_none() {
    // OpenCode is mapped to HarnessKind::Unsupported and therefore has no
    // auth check command of its own.
    assert!(auth_check_command_for(Harness::OpenCode).is_none());
}

#[test]
fn auth_check_command_for_unknown_is_none() {
    // Harness::Unknown causes harness_kind to return Err; the helper still
    // returns None instead of panicking.
    assert!(auth_check_command_for(Harness::Unknown).is_none());
}
