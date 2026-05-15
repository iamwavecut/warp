use ai::agent::action::{RunAgentsAgentRunConfig, RunAgentsExecutionMode, RunAgentsRequest};
use ai::agent::action_result::{
    RunAgentsAgentOutcome, RunAgentsAgentOutcomeKind, RunAgentsLaunchedExecutionMode,
    RunAgentsResult,
};
use ai::skills::SkillReference;
use std::path::PathBuf;

use super::RunAgentsEditState;
use crate::ai::blocklist::inline_action::orchestration_controls::OrchestrationEditState;

fn make_request(harness: &str, mode: RunAgentsExecutionMode) -> RunAgentsRequest {
    make_request_with_skills(harness, mode, Vec::new())
}

fn make_request_with_skills(
    harness: &str,
    mode: RunAgentsExecutionMode,
    skills: Vec<SkillReference>,
) -> RunAgentsRequest {
    RunAgentsRequest {
        summary: "summary".to_string(),
        base_prompt: "base".to_string(),
        skills,
        model_id: "auto".to_string(),
        harness_type: harness.to_string(),
        execution_mode: mode,
        agent_run_configs: vec![RunAgentsAgentRunConfig {
            name: "child".to_string(),
            prompt: "do work".to_string(),
            title: "Child agent".to_string(),
        }],
        plan_id: String::new(),
    }
}

fn make_edit_state_with_orch_fields(
    harness: &str,
    mode: RunAgentsExecutionMode,
) -> RunAgentsEditState {
    let request = make_request(harness, mode);
    RunAgentsEditState {
        orch: OrchestrationEditState::from_run_agents_fields(
            &request.model_id,
            &request.harness_type,
            &request.execution_mode,
        ),
        agent_run_configs: request.agent_run_configs,
        base_prompt: request.base_prompt,
        summary: request.summary,
        skills: request.skills,
        plan_id: request.plan_id,
    }
}

#[test]
fn remote_request_is_normalized_to_local() {
    let state = RunAgentsEditState::from_request(&make_request(
        "opencode",
        RunAgentsExecutionMode::Remote {
            environment_id: "env-1".to_string(),
            worker_host: "warp".to_string(),
            computer_use_enabled: false,
        },
    ));
    assert!(matches!(
        state.orch.execution_mode,
        RunAgentsExecutionMode::Local
    ));
    assert_eq!(state.orch.harness_type, "opencode");
    assert!(state.orch.accept_disabled_reason().is_none());
}

#[test]
fn local_with_any_harness_does_not_disable_accept() {
    for harness in ["oz", "gemini", "opencode"] {
        let state =
            RunAgentsEditState::from_request(&make_request(harness, RunAgentsExecutionMode::Local));
        assert!(
            state.orch.accept_disabled_reason().is_none(),
            "Local + {harness} should allow Accept"
        );
    }
}

#[test]
fn to_request_round_trips_request_fields() {
    let mut req = make_request_with_skills(
        "claude",
        RunAgentsExecutionMode::Remote {
            environment_id: "env-2".to_string(),
            worker_host: "warp".to_string(),
            computer_use_enabled: true,
        },
        vec![
            SkillReference::BundledSkillId("writing-pr-descriptions".to_string()),
            SkillReference::Path(PathBuf::from("/tmp/skill/SKILL.md")),
        ],
    );
    req.plan_id = "plan-1".to_string();
    let state = RunAgentsEditState::from_request(&req);
    let round_tripped = state.to_request();
    assert_eq!(round_tripped.summary, req.summary);
    assert_eq!(round_tripped.base_prompt, req.base_prompt);
    assert_eq!(round_tripped.model_id, req.model_id);
    assert_eq!(round_tripped.harness_type, req.harness_type);
    assert!(matches!(
        round_tripped.execution_mode,
        RunAgentsExecutionMode::Local
    ));
    assert_eq!(round_tripped.agent_run_configs, req.agent_run_configs);
    assert_eq!(round_tripped.skills, req.skills);
    assert_eq!(round_tripped.plan_id, req.plan_id);
}

mod format_terminal_state_tests {
    use super::super::{format_terminal_state, StatusKind};
    use super::*;

    fn launched(name: &str, agent_id: &str) -> RunAgentsAgentOutcome {
        RunAgentsAgentOutcome {
            name: name.to_string(),
            kind: RunAgentsAgentOutcomeKind::Launched {
                agent_id: agent_id.to_string(),
            },
        }
    }

    fn failed(name: &str, error: &str) -> RunAgentsAgentOutcome {
        RunAgentsAgentOutcome {
            name: name.to_string(),
            kind: RunAgentsAgentOutcomeKind::Failed {
                error: error.to_string(),
            },
        }
    }

    fn launched_result(agents: Vec<RunAgentsAgentOutcome>) -> RunAgentsResult {
        RunAgentsResult::Launched {
            model_id: "auto".to_string(),
            harness_type: "oz".to_string(),
            execution_mode: RunAgentsLaunchedExecutionMode::Local,
            agents,
        }
    }

    #[test]
    fn launched_singular_uses_singular_label() {
        let result = launched_result(vec![launched("child", "a-1")]);
        let (label, kind) = format_terminal_state(&result);
        assert_eq!(label, "Spawned 1 agent");
        assert!(matches!(kind, StatusKind::Success));
    }

    #[test]
    fn launched_plural_uses_plural_label() {
        let result = launched_result(vec![
            launched("a", "a-1"),
            launched("b", "a-2"),
            launched("c", "a-3"),
        ]);
        let (label, kind) = format_terminal_state(&result);
        assert_eq!(label, "Spawned 3 agents");
        assert!(matches!(kind, StatusKind::Success));
    }

    #[test]
    fn launched_partial_uses_x_of_y_label_and_mixed_status() {
        let result = launched_result(vec![
            launched("a", "a-1"),
            failed("b", "boom"),
            launched("c", "a-3"),
        ]);
        let (label, kind) = format_terminal_state(&result);
        assert_eq!(label, "Spawned 2 of 3 agents");
        assert!(matches!(kind, StatusKind::Mixed));
    }

    #[test]
    fn failure_with_error_includes_error_text() {
        let (label, kind) = format_terminal_state(&RunAgentsResult::Failure {
            error: "server rejected request".to_string(),
        });
        assert_eq!(
            label,
            "Failed to start orchestration: server rejected request"
        );
        assert!(matches!(kind, StatusKind::Failure));
    }

    #[test]
    fn failure_with_empty_error_uses_short_label() {
        let (label, kind) = format_terminal_state(&RunAgentsResult::Failure {
            error: String::new(),
        });
        assert_eq!(label, "Failed to start orchestration");
        assert!(matches!(kind, StatusKind::Failure));
    }

    #[test]
    fn denied_with_reason_appends_reason() {
        let (label, kind) = format_terminal_state(&RunAgentsResult::Denied {
            reason: "disapproved".to_string(),
        });
        assert!(label.contains("disapproved"));
        assert!(matches!(kind, StatusKind::Cancelled));
    }

    #[test]
    fn denied_without_reason_uses_short_label() {
        let (label, kind) = format_terminal_state(&RunAgentsResult::Denied {
            reason: String::new(),
        });
        assert!(!label.contains("()"));
        assert!(matches!(kind, StatusKind::Cancelled));
    }

    #[test]
    fn cancelled_uses_cancelled_status() {
        let (label, kind) = format_terminal_state(&RunAgentsResult::Cancelled);
        assert_eq!(label, "Spawn agents cancelled");
        assert!(matches!(kind, StatusKind::Cancelled));
    }
}

mod override_from_approved_config_tests {
    use super::super::RunAgentsEditState;
    use super::*;
    use ai::agent::orchestration_config::{OrchestrationConfig, OrchestrationExecutionMode};

    fn local_config(model: &str, harness: &str) -> OrchestrationConfig {
        OrchestrationConfig {
            model_id: model.to_string(),
            harness_type: harness.to_string(),
            execution_mode: OrchestrationExecutionMode::Local,
        }
    }

    fn remote_config(model: &str, harness: &str, env: &str) -> OrchestrationConfig {
        OrchestrationConfig {
            model_id: model.to_string(),
            harness_type: harness.to_string(),
            execution_mode: OrchestrationExecutionMode::Remote {
                environment_id: env.to_string(),
                worker_host: "warp".to_string(),
            },
        }
    }

    #[test]
    fn overrides_model_and_harness_unconditionally() {
        let mut state =
            RunAgentsEditState::from_request(&make_request("oz", RunAgentsExecutionMode::Local));
        assert_eq!(state.orch.model_id, "auto");
        assert_eq!(state.orch.harness_type, "oz");

        state
            .orch
            .override_from_approved_config(&local_config("claude-4-opus", "claude"));
        assert_eq!(state.orch.model_id, "claude-4-opus");
        assert_eq!(state.orch.harness_type, "claude");
    }

    #[test]
    fn overrides_even_when_request_has_values() {
        let mut state = RunAgentsEditState::from_request(&make_request(
            "claude",
            RunAgentsExecutionMode::Local,
        ));
        state
            .orch
            .override_from_approved_config(&local_config("gpt-5", "codex"));
        assert_eq!(state.orch.model_id, "gpt-5");
        assert_eq!(state.orch.harness_type, "codex");
    }

    #[test]
    fn approved_remote_config_keeps_execution_local() {
        let mut state =
            RunAgentsEditState::from_request(&make_request("oz", RunAgentsExecutionMode::Local));
        state
            .orch
            .override_from_approved_config(&remote_config("auto", "oz", "env-1"));
        assert!(matches!(
            state.orch.execution_mode,
            RunAgentsExecutionMode::Local
        ));
    }

    #[test]
    fn overrides_remote_to_local() {
        let mut state = RunAgentsEditState::from_request(&make_request(
            "oz",
            RunAgentsExecutionMode::Remote {
                environment_id: "env-1".to_string(),
                worker_host: "warp".to_string(),
                computer_use_enabled: true,
            },
        ));
        state
            .orch
            .override_from_approved_config(&local_config("auto", "oz"));
        assert!(
            matches!(state.orch.execution_mode, RunAgentsExecutionMode::Local),
            "should be Local after override"
        );
    }
}

mod compute_is_denied_tests {
    use super::super::compute_is_denied;
    use ai::agent::orchestration_config::{
        OrchestrationConfig, OrchestrationConfigStatus, OrchestrationExecutionMode,
    };

    fn some_config(
        status: OrchestrationConfigStatus,
    ) -> Option<(OrchestrationConfig, OrchestrationConfigStatus)> {
        Some((
            OrchestrationConfig {
                model_id: "auto".to_string(),
                harness_type: "oz".to_string(),
                execution_mode: OrchestrationExecutionMode::Local,
            },
            status,
        ))
    }

    #[test]
    fn false_when_no_denied_result_and_no_config() {
        assert!(!compute_is_denied(false, &None));
    }

    #[test]
    fn true_when_has_denied_result_from_history() {
        assert!(compute_is_denied(true, &None));
    }

    #[test]
    fn true_when_config_is_disapproved() {
        let config = some_config(OrchestrationConfigStatus::Disapproved);
        assert!(compute_is_denied(false, &config));
    }

    #[test]
    fn true_when_both_denied_and_disapproved() {
        let config = some_config(OrchestrationConfigStatus::Disapproved);
        assert!(compute_is_denied(true, &config));
    }

    #[test]
    fn false_when_config_is_approved() {
        let config = some_config(OrchestrationConfigStatus::Approved);
        assert!(!compute_is_denied(false, &config));
    }

    #[test]
    fn false_when_config_status_is_none() {
        let config = some_config(OrchestrationConfigStatus::None);
        assert!(!compute_is_denied(false, &config));
    }

    #[test]
    fn denied_result_overrides_approved_config() {
        let config = some_config(OrchestrationConfigStatus::Approved);
        assert!(
            compute_is_denied(true, &config),
            "History denied result should take precedence over approved config"
        );
    }
}
