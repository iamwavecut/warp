use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::ai::local_child_harnesses::local_child_harness_disabled_message;
use crate::ai::{
    agent_sdk::{
        driver::{
            AgentDriverError,
            harness::{
                HarnessKind, LocalHarnessRecord, LocalHarnessRepository, LocalHarnessSavePoint,
                claude_code::prepare_claude_environment_config, harness_kind,
                harness_model_env_vars,
            },
        },
        task_env_vars, validate_cli_installed,
    },
    ambient_agents::{AmbientAgentTaskId, task::HarnessModelConfig},
};
use crate::terminal::cli_agent_sessions::{
    CLIAgentSessionStatus, CLIAgentSessionsModel, CLIAgentSessionsModelEvent,
};
use crate::terminal::shell::ShellType;
use shell_words::quote as shell_quote;
use uuid::Uuid;
use warp_cli::agent::Harness;
use warpui::{EntityId, SingletonEntity, ViewContext};

use crate::pane_group::PaneGroup;

#[derive(Clone)]
pub(super) struct PreparedLocalHarnessLaunch {
    pub command: String,
    pub env_vars: HashMap<OsString, OsString>,
    pub run_id: String,
    pub task_id: AmbientAgentTaskId,
    pub harness: Harness,
    pub working_dir: PathBuf,
    pub session_id: Option<Uuid>,
}

pub(super) fn normalize_local_child_harness(harness_type: &str) -> Option<Harness> {
    Harness::parse_local_child_harness(harness_type)
}

pub(super) fn validate_local_harness_shell(shell_type: Option<ShellType>) -> Result<(), String> {
    match shell_type {
        Some(ShellType::Bash) | Some(ShellType::Zsh) | Some(ShellType::Fish) => Ok(()),
        Some(ShellType::PowerShell) => Err(
            "Local child harnesses currently require bash, zsh, or fish; PowerShell is not supported."
                .to_string(),
        ),
        None => Err(
            "Local child harnesses currently require a detected bash, zsh, or fish session."
                .to_string(),
        ),
    }
}

pub(super) fn build_local_claude_child_command(prompt: &str) -> String {
    let session_id = Uuid::new_v4();
    build_local_claude_child_command_with_session(prompt, session_id)
}

fn build_local_claude_child_command_with_session(prompt: &str, session_id: Uuid) -> String {
    let quoted_prompt = shell_quote(prompt);
    // Local child harness panes are launched off-screen. We intentionally skip
    // Claude's own permission prompts here so the child can start unattended
    // instead of hanging on an approval UI the user cannot see in that hidden
    // pane.
    format!("claude --session-id {session_id} --dangerously-skip-permissions {quoted_prompt}")
}

pub(super) fn build_local_opencode_child_command(prompt: &str) -> String {
    let quoted_prompt = shell_quote(prompt);
    format!("opencode --prompt {quoted_prompt}")
}
pub(super) fn build_local_codex_child_command(prompt: &str) -> String {
    let quoted_prompt = shell_quote(prompt);
    format!("codex --dangerously-bypass-approvals-and-sandbox {quoted_prompt}")
}

pub(super) async fn prepare_local_harness_child_launch(
    prompt: String,
    harness_type: String,
    model_id: Option<String>,
    parent_run_id: Option<String>,
    shell_type: Option<ShellType>,
    startup_directory: Option<PathBuf>,
) -> Result<PreparedLocalHarnessLaunch, String> {
    let harness_model_config =
        model_id
            .filter(|id| !id.is_empty())
            .map(|model_id| HarnessModelConfig {
                model_id,
                reasoning_level: None,
            });
    let Some(harness) = normalize_local_child_harness(&harness_type) else {
        let harness_name = harness_type.trim();
        return Err(if harness_name.is_empty() {
            "Local child harness type is missing.".to_string()
        } else {
            format!("Unsupported local child harness '{harness_name}'.")
        });
    };
    if let Some(message) = local_child_harness_disabled_message(harness) {
        return Err(message.to_string());
    }
    validate_local_harness_shell(shell_type)?;
    let startup_directory = startup_directory
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Could not resolve a working directory for the local child.".to_string())?;
    let working_dir = dunce::canonicalize(&startup_directory)
        .map_err(|error| format!("Could not resolve {}: {error}", startup_directory.display()))?;
    let task_id = AmbientAgentTaskId::generate();
    let run_id = Uuid::new_v4();
    let mut session_id = None;
    let command = match harness {
        Harness::Oz => unreachable!("normalize_local_child_harness filters out Oz"),
        Harness::Unknown => unreachable!("normalize_local_child_harness filters out Unknown"),
        Harness::Claude => {
            let HarnessKind::ThirdParty(third_party_harness) =
                harness_kind(harness).map_err(|error: AgentDriverError| error.to_string())?
            else {
                unreachable!("Claude resolves to a third-party harness")
            };
            third_party_harness
                .validate()
                .map_err(|error: AgentDriverError| error.to_string())?;
            // Local child harness panes inherit the user's existing local
            // auth/session state. We still prepare harness config files here,
            // but there are no Warp-managed secrets to materialize into the
            // hidden child pane.
            prepare_claude_environment_config(&working_dir, &HashMap::new())
                .map_err(|error| error.to_string())?;
            let claude_session_id = Uuid::new_v4();
            session_id = Some(claude_session_id);
            LocalHarnessRepository::for_user()
                .create(LocalHarnessRecord::new(
                    run_id,
                    harness,
                    claude_session_id,
                    &working_dir,
                    None,
                    Some(task_id),
                ))
                .map_err(|error| format!("Failed to create local harness record: {error}"))?;
            build_local_claude_child_command_with_session(&prompt, claude_session_id)
        }
        Harness::Codex => {
            let HarnessKind::ThirdParty(third_party_harness) =
                harness_kind(harness).map_err(|error: AgentDriverError| error.to_string())?
            else {
                unreachable!("Codex resolves to a third-party harness")
            };
            third_party_harness
                .validate()
                .map_err(|error: AgentDriverError| error.to_string())?;

            // Local Codex child panes must rely on the user's existing local
            // auth/session state. Do not run the shared Codex environment prep
            // here: it can seed OPENAI_API_KEY into ~/.codex/auth.json and
            // rewrite ~/.codex/config.toml for the whole machine.
            build_local_codex_child_command(&prompt)
        }
        Harness::OpenCode => {
            validate_cli_installed("opencode", Some("https://opencode.ai/docs"))
                .map_err(|error: AgentDriverError| error.to_string())?;
            build_local_opencode_child_command(&prompt)
        }
        Harness::Gemini => unreachable!("normalize_local_child_harness filters out Gemini"),
    };

    let mut env_vars = task_env_vars(Some(&task_id), parent_run_id.as_deref(), harness);
    // Propagate the selected model to Claude Code via ANTHROPIC_MODEL.
    // Codex local children never receive a model override — the UI
    // ensures model_id is empty for local Codex.
    env_vars.extend(harness_model_env_vars(
        harness,
        harness_model_config.as_ref(),
    ));

    Ok(PreparedLocalHarnessLaunch {
        command,
        env_vars,
        run_id: run_id.to_string(),
        task_id,
        harness,
        working_dir,
        session_id,
    })
}

/// Attach the local child pane to the same transcript/index lifecycle used by
/// standalone Codex and Claude runs. No AgentDriver, server API, or cloud
/// conversation state is involved in this callback.
pub(super) fn register_local_harness_child_lifecycle(
    terminal_view_id: EntityId,
    launch: &PreparedLocalHarnessLaunch,
    ctx: &mut ViewContext<PaneGroup>,
) {
    if !matches!(launch.harness, Harness::Codex | Harness::Claude) {
        return;
    }
    let repository = LocalHarnessRepository::for_user();
    let run_id = Uuid::parse_str(&launch.run_id).expect("prepared local run ID is a UUID");
    let task_id = launch.task_id;
    let harness = launch.harness;
    let working_dir = launch.working_dir.clone();
    let session_id = Arc::new(Mutex::new(launch.session_id));
    let session_id_for_event = Arc::clone(&session_id);
    ctx.subscribe_to_model(&CLIAgentSessionsModel::handle(ctx), move |_, _, event, ctx| {
        if event.terminal_view_id() != terminal_view_id {
            return;
        }
        let discovered_session_id = CLIAgentSessionsModel::as_ref(ctx)
            .session(terminal_view_id)
            .and_then(|session| session.session_context.session_id.as_deref())
            .and_then(|value| Uuid::parse_str(value).ok());
        if let Some(discovered_session_id) = discovered_session_id {
            *session_id_for_event.lock().expect("local child session mutex") =
                Some(discovered_session_id);
        }

        let (save_point, terminal) = match event {
            CLIAgentSessionsModelEvent::SessionUpdated { .. } => {
                (LocalHarnessSavePoint::PostTurn, false)
            }
            CLIAgentSessionsModelEvent::StatusChanged { status, .. }
                if matches!(status, CLIAgentSessionStatus::Success | CLIAgentSessionStatus::Failed { .. } | CLIAgentSessionStatus::Cancelled) =>
            {
                (LocalHarnessSavePoint::PostTurn, false)
            }
            CLIAgentSessionsModelEvent::Ended { .. } =>
                (LocalHarnessSavePoint::Final, true),
            _ => return,
        };
        let current_session_id = *session_id_for_event
            .lock()
            .expect("local child session mutex");
        if let Err(error) = persist_local_child_state(
            &repository,
            run_id,
            harness,
            task_id,
            &working_dir,
            current_session_id,
            save_point,
            terminal,
        ) {
            log::error!(
                "Failed to persist local {harness} child harness state for run {run_id}: {error}"
            );
        }
    });
}

fn persist_local_child_state(
    repository: &LocalHarnessRepository,
    run_id: Uuid,
    harness: Harness,
    task_id: AmbientAgentTaskId,
    working_dir: &Path,
    session_id: Option<Uuid>,
    save_point: LocalHarnessSavePoint,
    terminal: bool,
) -> Result<(), String> {
    let Some(session_id) = session_id else {
        if terminal {
            return Err("CLI ended before reporting a session UUID".to_owned());
        }
        return Ok(());
    };
    let mut record = match repository.read(run_id) {
        Ok(record) => record,
        Err(crate::ai::agent_sdk::driver::harness::LocalHarnessResumeError::MissingRecord {
            ..
        }) => repository
            .create(LocalHarnessRecord::new(
                run_id,
                harness,
                session_id,
                working_dir,
                None,
                Some(task_id),
            ))
            .map_err(|error| error.to_string())?,
        Err(error) => return Err(error.to_string()),
    };
    if record.harness != harness || record.harness_session_id != session_id {
        return Err("local child session does not match its indexed harness".to_owned());
    }
    let discovered = repository
        .discover_transcript(&record)
        .map_err(|error| error.to_string())?;
    let Some((locator, path)) = discovered else {
        record.last_save_point = Some(save_point);
        record.terminal = terminal;
        record.complete = terminal;
        let revision = record.revision;
        repository
            .update(record, revision)
            .map_err(|error| error.to_string())?;
        return if terminal {
            Err(format!(
                "{harness} transcript was not created before final save"
            ))
        } else {
            Ok(())
        };
    };
    if harness == Harness::Claude {
        repository
            .upsert_claude_sessions_index(&record, &path)
            .map_err(|error| error.to_string())?;
    }
    record.transcript = Some(locator);
    record.last_save_point = Some(save_point);
    record.terminal = terminal;
    record.complete = terminal;
    let revision = record.revision;
    repository
        .update(record, revision)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "local_harness_launch_tests.rs"]
mod tests;
