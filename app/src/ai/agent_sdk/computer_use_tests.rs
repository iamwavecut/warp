use super::*;

#[test]
fn text_only_named_runner_rejects_computer_use_before_provider_resolution() {
    warpui::App::test((), |app| async move {
        let config = AgentConfigSnapshot {
            computer_use_enabled: Some(true),
            ..Default::default()
        };
        let task = Task {
            computer_use_enabled: Some(true),
            prompt: AgentRunPrompt::Local("inspect desktop".to_owned()),
            model: None,
            profile: None,
            mcp_specs: Vec::new(),
            harness: HarnessKind::Oz,
            local_only: true,
        };
        let error = app
            .read(|ctx| preflight_named_execution(&config, &task, ctx))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Warp computer-use overrides are unavailable for local named-agent runs"
        );
    });
}
