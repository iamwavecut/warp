use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use shell_words::quote as shell_quote;
use uuid::Uuid;
use warp_cli::agent::Harness;

use crate::ai::agent::conversation::AIConversation;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::server::server_api::ServerApi;
use crate::terminal::CLIAgent;

use super::super::claude_transcript::{
    claude_config_dir, write_envelope, write_session_index_entry, ClaudeTranscriptEnvelope,
};
use super::super::task_env_vars;
use super::parent_bridge::{
    acknowledge_parent_bridge_hook_output, ensure_parent_bridge_state_dir, parent_bridge_root,
};
use super::{claude_command, prepare_claude_environment_config, ClaudeHarness};

const CLAUDE_WAKE_PROMPT: &str =
    "New lead-agent messages are available. Read the latest lead-agent updates and continue the task accordingly.";
pub(super) const CLAUDE_WAKE_PROMPT_FILE_NAME: &str = "wake-turn-prompt.txt";
const CLAUDE_WAKE_EXTERNALLY_MANAGED_LISTENER_ENV_VARS: &[&str] = &[
    "OZ_MESSAGE_LISTENER_MANAGED_EXTERNALLY",
    "OZ_PARENT_LISTENER_MANAGED_EXTERNALLY",
];

#[derive(Debug)]
pub(super) struct ClaudeWakeRemoteContext {
    pub(super) session_id: Uuid,
    pub(super) envelope: ClaudeTranscriptEnvelope,
    pub(super) wake_prompt: String,
}

impl ClaudeHarness {
    pub(crate) async fn wake_dormant_session(
        server_api: Arc<ServerApi>,
        conversation: AIConversation,
        parent_conversation: Option<AIConversation>,
        working_dir: Option<PathBuf>,
    ) -> Result<Option<String>> {
        let _ = (server_api, conversation, parent_conversation, working_dir);
        Ok(None)
    }

    pub(super) async fn prepare_local_wake_command(
        server_api: Arc<ServerApi>,
        task_id: AmbientAgentTaskId,
        parent_run_id: Option<String>,
        working_dir: Option<PathBuf>,
        mut remote: ClaudeWakeRemoteContext,
    ) -> Result<String> {
        let working_dir = working_dir.unwrap_or_else(|| remote.envelope.cwd.clone());
        prepare_claude_environment_config(&working_dir, &HashMap::new())
            .context("Failed to prepare Claude environment for wake")?;

        remote.envelope.cwd = working_dir.clone();
        let config_root = claude_config_dir().context("Failed to resolve Claude config dir")?;
        write_envelope(&remote.envelope, &config_root)
            .context("Failed to rehydrate Claude transcript for wake")?;
        if let Err(error) = write_session_index_entry(remote.session_id, &working_dir, &config_root)
        {
            log::warn!("Failed to update Claude sessions-index.json for wake: {error:#}");
        }

        let state_dir = parent_bridge_root()?.join(remote.session_id.to_string());
        ensure_parent_bridge_state_dir(&state_dir)?;
        let _ = server_api;
        acknowledge_parent_bridge_hook_output(&state_dir).await?;
        let prompt_path = state_dir.join(CLAUDE_WAKE_PROMPT_FILE_NAME);
        std::fs::write(&prompt_path, remote.wake_prompt.as_bytes())
            .with_context(|| format!("Failed to write {}", prompt_path.display()))?;

        let command = claude_command(
            CLIAgent::Claude.command_prefix(),
            &remote.session_id,
            &prompt_path.display().to_string(),
            None,
            None,
            true,
        );
        let env_vars = local_wake_task_env_vars(Some(&task_id), parent_run_id.as_deref());

        Ok(prefix_command_with_env_vars(command, env_vars))
    }
}

fn local_wake_task_env_vars(
    task_id: Option<&AmbientAgentTaskId>,
    parent_run_id: Option<&str>,
) -> HashMap<OsString, OsString> {
    let mut env_vars = task_env_vars(task_id, parent_run_id, Harness::Claude);
    // The local wake command is executed directly in the existing child
    // terminal, not through `AgentDriver::run_harness`, so Warp does not start
    // `MessageBridge` for this resumed Claude process. Leave the listener in
    // the Claude plugin's self-managed mode; otherwise the hook waits for
    // state files that no managed bridge is producing and the wake message is
    // never surfaced to Claude.
    for env_name in CLAUDE_WAKE_EXTERNALLY_MANAGED_LISTENER_ENV_VARS {
        env_vars.remove(OsStr::new(env_name));
    }
    env_vars
}

fn prefix_command_with_env_vars(command: String, env_vars: HashMap<OsString, OsString>) -> String {
    if env_vars.is_empty() {
        return command;
    }

    let mut env_pairs = env_vars
        .into_iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect::<Vec<_>>();
    env_pairs.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let assignments = env_pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", shell_quote(&value)))
        .collect::<Vec<_>>()
        .join(" ");

    format!("env {assignments} {command}")
}
