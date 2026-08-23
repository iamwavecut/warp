use super::*;
use clap::Parser;

/// Locks in [`Harness::config_name`] / [`Harness::from_config_name`] as a true inverse pair
/// for every variant that maps to a real, server-recognized harness. If a new variant is
/// added without a matching `from_config_name` arm, this round-trip test will fail.
#[test]
fn harness_config_name_round_trips_for_known_variants() {
    for harness in [
        Harness::Oz,
        Harness::Claude,
        Harness::OpenCode,
        Harness::Gemini,
        Harness::Codex,
    ] {
        assert_eq!(
            Harness::from_config_name(harness.config_name()),
            Some(harness),
            "round-trip failed for {harness:?}",
        );
    }
}

#[test]
fn harness_from_config_name_returns_none_for_unrecognized() {
    assert_eq!(Harness::from_config_name(""), None);
    assert_eq!(Harness::from_config_name("not-a-real-harness"), None);
}

#[test]
fn harness_from_config_name_round_trips_unknown() {
    assert_eq!(
        Harness::from_config_name(Harness::Unknown.config_name()),
        Some(Harness::Unknown),
    );
}

#[test]
fn named_agent_run_accepts_agent_selector_without_prompt() {
    let args = crate::Args::try_parse_from(["oz", "agent", "run", "--agent", "reviewer"])
        .expect("named agent selector should satisfy the run prompt group");

    let crate::Command::CommandLine(command) = args.command().expect("command") else {
        panic!("expected command line invocation");
    };
    let crate::CliCommand::Agent(AgentCommand::Run(run)) = command.as_ref() else {
        panic!("expected agent run");
    };
    assert_eq!(run.agent.as_deref(), Some("reviewer"));
    assert!(run.prompt_arg.prompt.is_none());
}

#[test]
fn named_agent_crud_commands_are_local_subcommands() {
    for argv in [
        &[
            "oz",
            "agent",
            "create",
            "--name",
            "reviewer",
            "--model",
            "custom/local/code",
        ][..],
        &["oz", "agent", "show", "reviewer"][..],
        &[
            "oz",
            "agent",
            "update",
            "reviewer",
            "--revision",
            "abc",
            "--name",
            "new",
        ][..],
        &[
            "oz",
            "agent",
            "delete",
            "reviewer",
            "--revision",
            "abc",
            "--yes",
        ][..],
    ] {
        crate::Args::try_parse_from(argv).expect("named-agent command should parse");
    }
}
