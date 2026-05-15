//! Ambient agent task types and utilities.

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use warp_cli::agent::Harness;
use warp_core::report_error;

use crate::ai::artifacts::{deserialize_artifacts, Artifact};
use crate::view_components::DismissibleToast;
use crate::workspace::ToastStack;
use warpui::{SingletonEntity, View, ViewContext};

use super::AmbientAgentTaskId;

/// Runtime configuration snapshot for agent execution.
///
/// This is the merged/resolved config used when spawning or running an agent.
/// It combines settings from config files and CLI args.
/// Unlike `AgentConfig` (the cloud model), field names here use the runtime format
/// (e.g. `model_id` instead of `base_model_id`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentConfigSnapshot {
    /// Config name for searchability/traceability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_prompt: Option<String>,
    /// MCP server configuration map (unwrapped; no `mcpServers` wrapper).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<serde_json::Map<String, serde_json::Value>>,
    /// Profile ID for local agent runs. This configures the terminal session
    /// with the specified execution profile. Only used for local runs, not cloud runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Self-hosted worker ID that should execute this task.
    /// Retained for parsing older configs; hosted worker dispatch is disabled in this fork.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_host: Option<String>,
    /// Skill spec to use as the base prompt for the agent.
    /// Format: "skill_name", "repo:skill_name", or "org/repo:skill_name".
    /// The skill is resolved at runtime in the agent environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_spec: Option<String>,
    /// Whether computer use is enabled for this agent run.
    /// If None, the default behavior is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_use_enabled: Option<bool>,
    /// Execution harness for the agent run.
    /// If None, we use Warp's default ("oz").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessConfig>,
    /// Authentication secrets for third-party harnesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_auth_secrets: Option<HarnessAuthSecretsConfig>,
}

/// Configuration for a third-party execution harness.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HarnessConfig {
    /// The harness type, e.g. [`Harness::Claude`].
    #[serde(
        rename = "type",
        serialize_with = "serialize_harness",
        deserialize_with = "deserialize_harness"
    )]
    pub harness_type: Harness,
}

impl HarnessConfig {
    /// Builds a harness config from just the harness type.
    pub fn from_harness_type(harness_type: Harness) -> Self {
        Self { harness_type }
    }
}

fn serialize_harness<S: Serializer>(harness: &Harness, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(harness.config_name())
}

fn deserialize_harness<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Harness, D::Error> {
    let name = String::deserialize(deserializer)?;
    Ok(Harness::from_config_name(&name).unwrap_or_else(|| {
        log::warn!("Unknown harness config name: {name:?}; treating as Unknown");
        Harness::Unknown
    }))
}

/// Authentication secrets for third-party harnesses.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HarnessAuthSecretsConfig {
    /// Name of a managed secret for Claude Code harness authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_auth_secret_name: Option<String>,
    /// Name of a managed secret for Codex harness authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_auth_secret_name: Option<String>,
}

impl AgentConfigSnapshot {
    /// Returns true if this config is empty (no options are set).
    pub fn is_empty(&self) -> bool {
        let Self {
            name,
            environment_id,
            model_id,
            base_prompt,
            mcp_servers,
            profile_id,
            worker_host,
            skill_spec,
            computer_use_enabled,
            harness,
            harness_auth_secrets,
        } = self;

        name.is_none()
            && environment_id.is_none()
            && model_id.is_none()
            && base_prompt.is_none()
            && mcp_servers.is_none()
            && profile_id.is_none()
            && worker_host.is_none()
            && skill_spec.is_none()
            && computer_use_enabled.is_none()
            && harness.is_none()
            && harness_auth_secrets.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSource {
    Cli,
    Interactive,
}

impl AgentSource {
    pub fn as_str(&self) -> &str {
        match self {
            AgentSource::Cli => "CLI",
            AgentSource::Interactive => "LOCAL",
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            AgentSource::Cli => "CLI",
            AgentSource::Interactive => "Warp App",
        }
    }

    /// Returns true if this source represents a user-initiated conversation.
    pub fn is_user_initiated(&self) -> bool {
        match self {
            AgentSource::Interactive => true,
            AgentSource::Cli => false,
        }
    }
}

fn deserialize_ambient_agent_source<'de, D>(
    deserializer: D,
) -> Result<Option<AgentSource>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: Option<String> = serde::Deserialize::deserialize(deserializer)?;
    Ok(match s {
        Some(s) => match s.as_str() {
            "LOCAL" | "CLOUD_MODE" => Some(AgentSource::Interactive),
            "CLI" => Some(AgentSource::Cli),
            _ => {
                report_error!(anyhow!("Unknown AmbientAgentSource: {}", s));
                None
            }
        },
        None => None,
    })
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct AmbientAgentTask {
    pub task_id: AmbientAgentTaskId,
    #[serde(default)]
    pub parent_run_id: Option<String>,
    pub title: String,
    pub state: AmbientAgentTaskState,
    pub prompt: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub status_message: Option<TaskStatusMessage>,
    #[serde(default, deserialize_with = "deserialize_ambient_agent_source")]
    pub source: Option<AgentSource>,
    pub session_id: Option<String>,
    pub session_link: Option<String>,
    pub creator: Option<TaskPrincipalInfo>,
    #[serde(default)]
    pub executor: Option<TaskPrincipalInfo>,
    pub conversation_id: Option<String>,
    pub request_usage: Option<RequestUsage>,
    pub is_sandbox_running: bool,

    /// Snapshot of the agent config used to create the task.
    #[serde(default, alias = "agent_config")]
    pub agent_config_snapshot: Option<AgentConfigSnapshot>,
    #[serde(default, deserialize_with = "deserialize_artifacts")]
    pub artifacts: Vec<Artifact>,

    /// The last event sequence number recorded for this local run.
    /// Used by orchestration event delivery to resume from the correct
    /// cursor on restart.
    #[serde(default)]
    pub last_event_sequence: Option<i64>,

    /// The locally recorded `run_id`s of direct children of this run.
    #[serde(default)]
    pub children: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunExecution<'a> {
    pub session_id: Option<&'a str>,
    pub session_link: Option<&'a str>,
    pub request_usage: Option<&'a RequestUsage>,
    pub is_sandbox_running: bool,
}

impl RunExecution<'_> {
    pub fn has_joinable_session(&self) -> bool {
        self.session_id.is_some() || self.session_link.is_some()
    }

    pub fn is_active(&self) -> bool {
        self.is_sandbox_running && self.has_joinable_session()
    }
}

/// Represents a single attachment input from the client (e.g., file upload)
#[derive(Clone, Debug, Serialize)]
pub struct AttachmentInput {
    pub file_name: String,
    pub mime_type: String,
    pub data: String, // base64-encoded data
}

impl AmbientAgentTask {
    pub fn run_id(&self) -> AmbientAgentTaskId {
        self.task_id
    }

    pub fn conversation_id(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    pub fn active_run_execution(&self) -> RunExecution<'_> {
        RunExecution {
            session_id: self.session_id.as_deref(),
            session_link: self.session_link.as_deref().filter(|link| !link.is_empty()),
            request_usage: self.request_usage.as_ref(),
            is_sandbox_running: self.is_sandbox_running,
        }
    }

    pub fn active_execution_session_id(&self) -> Option<&str> {
        let execution = self.active_run_execution();
        if self.state == AmbientAgentTaskState::InProgress && execution.is_active() {
            execution.session_id
        } else {
            None
        }
    }

    pub fn has_active_execution(&self) -> bool {
        self.state == AmbientAgentTaskState::InProgress && self.active_run_execution().is_active()
    }

    /// Total credits used (inference + compute + platform).
    pub fn credits_used(&self) -> Option<f32> {
        self.active_run_execution().request_usage.map(|u| {
            (u.inference_cost.unwrap_or(0.0)
                + u.compute_cost.unwrap_or(0.0)
                + u.platform_cost.unwrap_or(0.0)) as f32
        })
    }

    /// Duration from started_at to updated_at.
    pub fn run_time(&self) -> Option<chrono::Duration> {
        let started = self.started_at?;
        let duration = self.updated_at.signed_duration_since(started);
        (duration.num_seconds() >= 0).then_some(duration)
    }

    /// Creator's display name, if available.
    pub fn creator_display_name(&self) -> Option<String> {
        self.creator.as_ref().and_then(|c| c.display_name.clone())
    }

    /// Returns true if the underlying session for the ambient agent is no longer running.
    pub fn is_no_longer_running(&self) -> bool {
        !self.active_run_execution().is_sandbox_running && !self.state.is_working()
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum AmbientAgentTaskState {
    Queued,
    Pending,
    Claimed,
    #[serde(alias = "IN_PROGRESS")]
    InProgress,
    Succeeded,
    Failed,
    Error,
    Blocked,
    Cancelled,
    #[serde(other)]
    Unknown,
}

impl AmbientAgentTaskState {
    pub fn is_working(&self) -> bool {
        match self {
            AmbientAgentTaskState::Queued
            | AmbientAgentTaskState::Pending
            | AmbientAgentTaskState::Claimed
            | AmbientAgentTaskState::InProgress => true,
            AmbientAgentTaskState::Succeeded
            | AmbientAgentTaskState::Failed
            | AmbientAgentTaskState::Error
            | AmbientAgentTaskState::Blocked
            | AmbientAgentTaskState::Cancelled
            | AmbientAgentTaskState::Unknown => false,
        }
    }

    pub fn is_failure_like(&self) -> bool {
        match self {
            AmbientAgentTaskState::Failed
            | AmbientAgentTaskState::Error
            | AmbientAgentTaskState::Blocked
            | AmbientAgentTaskState::Unknown => true,
            AmbientAgentTaskState::Queued
            | AmbientAgentTaskState::Pending
            | AmbientAgentTaskState::Claimed
            | AmbientAgentTaskState::InProgress
            | AmbientAgentTaskState::Succeeded
            | AmbientAgentTaskState::Cancelled => false,
        }
    }
}

impl std::fmt::Display for AmbientAgentTaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AmbientAgentTaskState::Queued => write!(f, "Queued"),
            AmbientAgentTaskState::Pending => write!(f, "Pending"),
            AmbientAgentTaskState::Claimed => write!(f, "Claimed"),
            AmbientAgentTaskState::InProgress => write!(f, "In progress"),
            AmbientAgentTaskState::Succeeded => write!(f, "Done"),
            AmbientAgentTaskState::Failed => write!(f, "Failed"),
            AmbientAgentTaskState::Error => write!(f, "Error"),
            AmbientAgentTaskState::Blocked => write!(f, "Blocked"),
            AmbientAgentTaskState::Cancelled => write!(f, "Cancelled"),
            AmbientAgentTaskState::Unknown => write!(f, "Failed"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct TaskPrincipalInfo {
    #[serde(rename = "type")]
    pub creator_type: String,
    pub uid: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct TaskStatusMessage {
    pub message: String,
    #[serde(default, alias = "errorCode")]
    pub error_code: Option<TaskStatusErrorCode>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatusErrorCode {
    #[serde(alias = "ENVIRONMENT_SETUP_FAILED")]
    EnvironmentSetupFailed,
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
impl TaskStatusErrorCode {
    pub fn is_environment_setup_failure(&self) -> bool {
        matches!(self, TaskStatusErrorCode::EnvironmentSetupFailed)
    }
}

#[cfg(test)]
impl TaskStatusMessage {
    pub fn is_environment_setup_failure(&self) -> bool {
        self.error_code
            .as_ref()
            .is_some_and(TaskStatusErrorCode::is_environment_setup_failure)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct RequestUsage {
    pub inference_cost: Option<f64>,
    pub compute_cost: Option<f64>,
    pub platform_cost: Option<f64>,
}

/// Cancel an ambient agent task and show a toast with the result.
pub fn cancel_task_with_toast<V: View>(task_id: AmbientAgentTaskId, ctx: &mut ViewContext<V>) {
    let _ = task_id;
    let window_id = ctx.window_id();
    ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
        let toast = DismissibleToast::default(
            "Hosted ambient task cancellation is disabled in this local-first build.".to_string(),
        );
        toast_stack.add_ephemeral_toast(toast, window_id, ctx);
    });
}

/// Cancel an ambient agent task without surfacing a toast to the user.
pub fn cancel_task_silently<V: View>(task_id: AmbientAgentTaskId, ctx: &mut ViewContext<V>) {
    let _ = (task_id, ctx);
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
