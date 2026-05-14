use super::*;
use clap::Parser;

use crate::agent::{AgentCommand, Harness, OutputFormat};

#[test]
fn agent_run_accepts_model() {
    let args = Args::try_parse_from([
        "warp", "agent", "run", "--prompt", "hello", "--model", "gpt-4o",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.model.model.as_deref(), Some("gpt-4o"));
}

#[test]
fn model_list_parses() {
    let args = Args::try_parse_from(["warp", "model", "list"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp model list` command");
    };
    let CliCommand::Model(model_cmd) = boxed_cmd.as_ref() else {
        panic!("Expected `warp model` command");
    };

    assert!(matches!(model_cmd, crate::model::ModelCommand::List));
}

#[test]
fn login_parses() {
    let args = Args::try_parse_from(["warp", "login"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp login` command");
    };

    assert!(matches!(boxed_cmd.as_ref(), CliCommand::Login));
}

#[test]
fn logout_parses() {
    let args = Args::try_parse_from(["warp", "logout"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp logout` command");
    };

    assert!(matches!(boxed_cmd.as_ref(), CliCommand::Logout));
}

#[test]
fn agent_run_accepts_file() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--file",
        "config.yaml",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(
        run_args.config_file.file.as_ref().and_then(|p| p.to_str()),
        Some("config.yaml")
    );
}

#[test]
fn agent_run_accepts_idle_on_complete_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--idle-on-complete",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(
        run_args.idle_on_complete,
        Some(humantime::Duration::from(std::time::Duration::from_secs(
            45 * 60
        )))
    );
}

#[test]
fn agent_run_accepts_idle_on_complete_duration() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--idle-on-complete",
        "10m",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(
        run_args.idle_on_complete,
        Some(humantime::Duration::from(std::time::Duration::from_secs(
            10 * 60
        )))
    );
}

#[test]
fn agent_run_cloud_accepts_file_short_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "-f",
        "config.json",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert_eq!(
        run_args.config_file.file.as_ref().and_then(|p| p.to_str()),
        Some("config.json")
    );
}

#[test]
fn agent_run_cloud_accepts_model() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--model",
        "gpt-4o",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert_eq!(run_args.model.model.as_deref(), Some("gpt-4o"));
}

#[test]
fn agent_run_cloud_accepts_mcp() {
    let uuid = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--mcp",
        "550e8400-e29b-41d4-a716-446655440000",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert!(matches!(
        run_args.mcp_specs.as_slice(),
        [crate::mcp::MCPSpec::Uuid(parsed_uuid)] if *parsed_uuid == uuid
    ));
}

#[test]
fn agent_run_cloud_accepts_run_ambient_alias() {
    // Ensure backwards compatibility: run-ambient should still work as an alias
    let args = Args::try_parse_from(["warp", "agent", "run-ambient", "--prompt", "hello"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-ambient` (alias) command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(_)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-ambient` to parse as RunCloud");
    };
}

#[test]
fn agent_run_rejects_prompt_and_task_id() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--task-id",
        "d1b9b002-a8e1-422a-9016-e62490cb6a59",
    ]);
    assert!(result.is_err());
}

#[test]
fn agent_run_rejects_without_prompt_or_task_id() {
    let result = Args::try_parse_from(["warp", "agent", "run", "--model", "gpt-4o"]);
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("prompt_group") || err_str.contains("required"));
}

#[test]
fn agent_run_accepts_prompt_only() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--prompt", "hello"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.prompt_arg.prompt.as_deref(), Some("hello"));
    assert!(run_args.prompt_arg.saved_prompt.is_none());
    assert!(run_args.skill.is_none());
    assert!(run_args.task_id.is_none());
}

#[test]
fn agent_run_accepts_saved_prompt_only() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--saved-prompt", "sp-123"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.prompt_arg.prompt.is_none());
    assert_eq!(run_args.prompt_arg.saved_prompt.as_deref(), Some("sp-123"));
    assert!(run_args.skill.is_none());
    assert!(run_args.task_id.is_none());
}

#[test]
fn agent_run_accepts_skill_only() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--skill", "my-skill"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.prompt_arg.prompt.is_none());
    assert!(run_args.skill.is_some());
    assert!(run_args.task_id.is_none());
}

#[test]
fn agent_run_accepts_task_id_only() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--task-id", "tid-456"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.prompt_arg.prompt.is_none());
    assert!(run_args.skill.is_none());
    assert_eq!(run_args.task_id.as_deref(), Some("tid-456"));
}

#[test]
fn agent_run_accepts_prompt_and_skill() {
    let args = Args::try_parse_from([
        "warp", "agent", "run", "--prompt", "do stuff", "--skill", "my-skill",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.prompt_arg.prompt.as_deref(), Some("do stuff"));
    assert!(run_args.skill.is_some());
}

#[test]
fn agent_run_accepts_saved_prompt_and_skill() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--saved-prompt",
        "sp-1",
        "--skill",
        "my-skill",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert_eq!(run_args.prompt_arg.saved_prompt.as_deref(), Some("sp-1"));
    assert!(run_args.skill.is_some());
}

#[test]
fn agent_run_rejects_saved_prompt_and_task_id() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--saved-prompt",
        "sp-1",
        "--task-id",
        "tid-1",
    ]);
    assert!(result.is_err());
}

#[test]
fn agent_run_rejects_file_and_task_id() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--task-id",
        "tid-1",
        "--file",
        "config.yaml",
    ]);
    assert!(result.is_err());
}

#[test]
fn agent_run_accepts_skill_and_task_id() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--skill",
        "my-skill",
        "--task-id",
        "tid-1",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.prompt_arg.prompt.is_none());
    assert!(run_args.skill.is_some());
    assert_eq!(run_args.task_id.as_deref(), Some("tid-1"));
}

#[test]
fn agent_run_rejects_prompt_and_saved_prompt() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--saved-prompt",
        "sp-1",
    ]);
    assert!(result.is_err());
}

#[test]
fn environment_subcommand_is_not_registered() {
    assert!(Args::try_parse_from(["warp", "environment", "image", "list"]).is_err());
}

#[test]
fn agent_run_accepts_computer_use_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--computer-use",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(run_args.computer_use.computer_use);
    assert!(!run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), Some(true));
}

#[test]
fn agent_run_accepts_no_computer_use_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--no-computer-use",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(!run_args.computer_use.computer_use);
    assert!(run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), Some(false));
}

#[test]
fn agent_run_rejects_both_computer_use_flags() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run",
        "--prompt",
        "hello",
        "--computer-use",
        "--no-computer-use",
    ]);

    assert!(result.is_err());
}

#[test]
fn agent_run_defaults_to_no_computer_use_override() {
    let args = Args::try_parse_from(["warp", "agent", "run", "--prompt", "hello"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run` command");
    };
    let CliCommand::Agent(AgentCommand::Run(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run` command");
    };

    assert!(!run_args.computer_use.computer_use);
    assert!(!run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), None);
}
#[test]
fn agent_run_cloud_accepts_computer_use_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--computer-use",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert!(run_args.computer_use.computer_use);
    assert!(!run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), Some(true));
}

#[test]
fn agent_run_cloud_accepts_no_computer_use_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--no-computer-use",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert!(!run_args.computer_use.computer_use);
    assert!(run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), Some(false));
}

#[test]
fn agent_run_cloud_rejects_both_computer_use_flags() {
    let result = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--computer-use",
        "--no-computer-use",
    ]);

    assert!(result.is_err());
}

#[test]
fn agent_run_cloud_defaults_to_no_computer_use_override() {
    let args = Args::try_parse_from(["warp", "agent", "run-cloud", "--prompt", "hello"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert!(!run_args.computer_use.computer_use);
    assert!(!run_args.computer_use.no_computer_use);
    assert_eq!(run_args.computer_use.computer_use_override(), None);
}

#[test]
fn agent_run_cloud_accepts_harness_flag() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--harness",
        "claude",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert_eq!(run_args.harness, Harness::Claude);
}

#[test]
fn agent_run_cloud_defaults_harness_to_oz() {
    let args = Args::try_parse_from(["warp", "agent", "run-cloud", "--prompt", "hello"]).unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert_eq!(run_args.harness, Harness::Oz);
}

#[test]
fn harness_parse_orchestration_harness_accepts_aliases() {
    assert_eq!(
        Harness::parse_orchestration_harness("claude-code"),
        Some(Harness::Claude)
    );
    assert_eq!(
        Harness::parse_orchestration_harness("open_code"),
        Some(Harness::OpenCode)
    );
}

#[test]
fn harness_parse_local_child_harness_rejects_oz() {
    assert_eq!(Harness::parse_local_child_harness("oz"), None);
    assert_eq!(
        Harness::parse_local_child_harness("opencode"),
        Some(Harness::OpenCode)
    );
}

#[test]
fn harness_parse_orchestration_harness_accepts_codex() {
    assert_eq!(
        Harness::parse_orchestration_harness("codex"),
        Some(Harness::Codex)
    );
}

#[test]
fn harness_parse_local_child_harness_accepts_codex() {
    assert_eq!(
        Harness::parse_local_child_harness("codex"),
        Some(Harness::Codex)
    );
}

#[test]
fn agent_run_cloud_accepts_claude_auth_secret_with_harness() {
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--harness",
        "claude",
        "--claude-auth-secret",
        "my-key",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert_eq!(run_args.harness, Harness::Claude);
    assert_eq!(run_args.claude_auth_secret.as_deref(), Some("my-key"));
}

#[test]
fn agent_run_cloud_claude_auth_secret_without_harness_parses() {
    // Clap parsing succeeds; runtime validation (in mod.rs) rejects this combination.
    let args = Args::try_parse_from([
        "warp",
        "agent",
        "run-cloud",
        "--prompt",
        "hello",
        "--claude-auth-secret",
        "my-key",
    ])
    .unwrap();

    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("Expected `warp agent run-cloud` command");
    };
    let CliCommand::Agent(AgentCommand::RunCloud(run_args)) = boxed_cmd.as_ref() else {
        panic!("Expected `warp agent run-cloud` command");
    };

    assert_eq!(run_args.harness, Harness::Oz);
    assert_eq!(run_args.claude_auth_secret.as_deref(), Some("my-key"));
}
