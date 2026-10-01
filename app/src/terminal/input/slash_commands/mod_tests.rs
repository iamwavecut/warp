use super::slash_command_is_submitted_as_prompt;
use crate::features::FeatureFlag;
use crate::search::slash_command_menu::static_commands::{Availability, commands};
const BASELINE_AVAILABILITY: Availability = Availability::AGENT_VIEW
    .union(Availability::AI_ENABLED)
    .union(Availability::NO_LRC_CONTROL);

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn open_file_command_uses_session_home_instead_of_host_home() {
    use std::sync::Arc;

    use super::open_file_command_path;
    use crate::terminal::model::session::command_executor::testing::TestCommandExecutor;
    use crate::terminal::model::session::{Session, SessionInfo};
    use crate::terminal::shell::ShellType;

    let session = Session::new(
        SessionInfo::new_for_test()
            .with_shell_type(ShellType::Bash)
            .with_home_dir("/home/session-user".to_owned()),
        Arc::new(TestCommandExecutor::default()),
    );
    let (path, line_col) = open_file_command_path(&session, "/work", "~/file\\ name.rs:4:2");
    assert_eq!(
        path,
        std::path::PathBuf::from("/home/session-user/file name.rs")
    );
    assert_eq!(
        line_col,
        Some(warp_util::path::LineAndColumnArg {
            line_num: 4,
            column_num: Some(2)
        })
    );
}

/// The centralized classifier must mark only the prompt-submitting commands (/compact, /plan,
/// /orchestrate) as "submitted as a prompt". Every other slash command emits an immediate action
/// and must be treated as "run now" by the prompt-queue gate and the shared-session viewer path.
#[test]
fn slash_command_is_submitted_as_prompt_only_for_prompt_commands() {
    // Prompt-submitting commands reiterate their text into the conversation.
    assert!(slash_command_is_submitted_as_prompt(&commands::COMPACT));
    assert!(slash_command_is_submitted_as_prompt(&commands::PLAN));
    assert!(slash_command_is_submitted_as_prompt(&commands::ORCHESTRATE));

    // Action-emitting commands run immediately and are never queued / forwarded as prompts.
    assert!(!slash_command_is_submitted_as_prompt(&commands::FORK));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::FORK_AND_COMPACT
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::FORK_FROM));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::COMPACT_AND
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::MODEL));
    assert!(!slash_command_is_submitted_as_prompt(&commands::REWIND));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::CONVERSATIONS
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::QUEUE));
}

#[test]
fn not_cloud_agent_commands_are_only_active_outside_cloud_mode() {
    let local_context = BASELINE_AVAILABILITY | Availability::NOT_AMBIENT_AGENT;
    assert!(commands::AGENT.is_active(local_context));
    assert!(commands::NEW.is_active(local_context));

    let cloud_context = BASELINE_AVAILABILITY;
    assert!(!commands::AGENT.is_active(cloud_context));
    assert!(!commands::NEW.is_active(cloud_context));

    let _cloud_mode_input_v2 = FeatureFlag::CloudModeInputV2.override_enabled(true);
    let ambient_agent_v2_context = BASELINE_AVAILABILITY | Availability::AMBIENT_AGENT_V2;
    assert!(!commands::AGENT.is_active(ambient_agent_v2_context));
    assert!(!commands::NEW.is_active(ambient_agent_v2_context));
}

#[test]
fn ambient_agent_v2_commands_are_active_only_in_ambient_agent_v2_context() {
    let cloud_context = BASELINE_AVAILABILITY;
    assert!(!commands::HARNESS.is_active(cloud_context));

    let _cloud_mode_input_v2 = FeatureFlag::CloudModeInputV2.override_enabled(true);
    let ambient_agent_v2_context = BASELINE_AVAILABILITY | Availability::AMBIENT_AGENT_V2;
    assert!(commands::PLAN.is_active(ambient_agent_v2_context));
    assert!(commands::MODEL.is_active(ambient_agent_v2_context));
    assert!(commands::HARNESS.is_active(ambient_agent_v2_context));
}

#[cfg(all(feature = "local_fs", windows))]
mod windows {
    use std::sync::Arc;

    use super::super::*;
    use crate::terminal::ShellLaunchData;
    use crate::terminal::model::session::SessionInfo;
    use crate::terminal::model::session::command_executor::testing::TestCommandExecutor;
    use crate::terminal::shell::ShellType;

    fn wsl_session() -> Session {
        Session::new(
            SessionInfo::new_for_test()
                .with_shell_type(ShellType::Bash)
                .with_home_dir("/home/ubuntu".to_owned()),
            Arc::new(TestCommandExecutor::default()),
        )
        .with_shell_launch_data(ShellLaunchData::WSL {
            distro: "Ubuntu".to_owned(),
        })
    }

    #[test]
    fn open_file_command_converts_wsl_paths_to_host_paths() {
        let session = wsl_session();
        let cases = [
            (
                "/home/ubuntu",
                "subdir/test.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\test.txt",
                None,
            ),
            (
                "/home/ubuntu/project",
                "../test.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\test.txt",
                None,
            ),
            (
                "/home/ubuntu",
                "subdir/file\\ name.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\file name.txt",
                None,
            ),
            (
                "/home/ubuntu",
                "subdir/test.txt:4:2",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\test.txt",
                Some(LineAndColumnArg {
                    line_num: 4,
                    column_num: Some(2),
                }),
            ),
            (
                "/tmp",
                "~/subdir/file\\ name.txt:4:2",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\file name.txt",
                Some(LineAndColumnArg {
                    line_num: 4,
                    column_num: Some(2),
                }),
            ),
        ];

        for (current_dir, raw_arg, expected_path, expected_line_col) in cases {
            let (path, line_col) = open_file_command_path(&session, current_dir, raw_arg);

            assert_eq!(path, PathBuf::from(expected_path));
            assert_eq!(line_col, expected_line_col);
        }
    }
}
