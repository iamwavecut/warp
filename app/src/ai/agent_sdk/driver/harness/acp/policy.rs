//! Checks local permission policy without granting unattended approval.
use std::path::PathBuf;

use warp_util::path::EscapeChar;
use warpui::{AppContext, EntityId, SingletonEntity};

use super::mapping::{command_from_raw_input, first_diff, path_from_raw_input};
use super::protocol::{self, ToolCallFields, ToolKind};
use crate::ai::blocklist::{
    BlocklistAIController, BlocklistAIPermissions, CommandExecutionPermission, FileWritePermission,
};

/// Something the agent wants to do that Warp's permission policy has an opinion on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PolicyRequest {
    Execute { command: String },
    WriteFiles { paths: Vec<PathBuf> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PolicyDecision {
    Allow,
    Deny { reason: String },
}

impl PolicyRequest {
    /// The policy-relevant part of a tool call the agent asked permission for, if any. The
    /// agent chooses which fields to include, so paths are gathered from every place it may
    /// have put them.
    pub(super) fn for_tool_call(call: &ToolCallFields) -> Option<Self> {
        match call.kind? {
            ToolKind::Execute => {
                let title = call.title.as_deref().unwrap_or_default();
                command_from_raw_input(call.raw_input.as_ref(), call.kind, title)
                    .map(|command| Self::Execute { command })
            }
            ToolKind::Edit | ToolKind::Delete | ToolKind::Move => {
                let mut paths: Vec<PathBuf> = call
                    .locations
                    .iter()
                    .flatten()
                    .map(|location| PathBuf::from(&location.path))
                    .collect();
                if let Some((path, _, _)) = first_diff(call.content.as_deref()) {
                    paths.push(PathBuf::from(path));
                }
                if let Some(path) = path_from_raw_input(call.raw_input.as_ref()) {
                    paths.push(PathBuf::from(path));
                }
                paths.retain(|path| !path.as_os_str().is_empty());
                paths.dedup();
                (!paths.is_empty()).then_some(Self::WriteFiles { paths })
            }
            ToolKind::Read
            | ToolKind::Search
            | ToolKind::Think
            | ToolKind::Fetch
            | ToolKind::SwitchMode
            | ToolKind::Other
            | ToolKind::Unknown => None,
        }
    }

    pub(super) fn write(path: &str) -> Self {
        Self::WriteFiles {
            paths: vec![PathBuf::from(path)],
        }
    }

    /// Evaluates against the permissions that apply to `controller`'s bound native
    /// conversation, which is the one the agent's turn is rendered through.
    pub(super) fn evaluate(
        &self,
        controller: &BlocklistAIController,
        terminal_view_id: EntityId,
        escape_char: EscapeChar,
        ctx: &AppContext,
    ) -> PolicyDecision {
        let Some(conversation_id) = controller.native_prompt_conversation_id() else {
            return PolicyDecision::Deny {
                reason: "no local conversation is bound".into(),
            };
        };
        let permissions = BlocklistAIPermissions::as_ref(ctx);
        match self {
            Self::Execute { command } => match permissions.can_autoexecute_command(
                &conversation_id,
                command,
                escape_char,
                false,
                None,
                Some(terminal_view_id),
                ctx,
            ) {
                CommandExecutionPermission::Denied(reason) => PolicyDecision::Deny {
                    reason: format!("command requires approval or is forbidden: {reason:?}"),
                },
                CommandExecutionPermission::Allowed(_) => PolicyDecision::Allow,
            },
            Self::WriteFiles { paths } => match permissions.can_write_files(
                &conversation_id,
                paths,
                Some(terminal_view_id),
                ctx,
            ) {
                FileWritePermission::Denied(reason) => PolicyDecision::Deny {
                    reason: format!("file write requires approval or is forbidden: {reason:?}"),
                },
                FileWritePermission::Allowed(_) => PolicyDecision::Allow,
            },
        }
    }
}

pub(super) fn select_permission(
    decision: &PolicyDecision,
    options: &[protocol::PermissionOption],
) -> protocol::PermissionOutcome {
    let kind = match decision {
        PolicyDecision::Allow => protocol::PermissionOptionKind::AllowOnce,
        PolicyDecision::Deny { .. } => protocol::PermissionOptionKind::RejectOnce,
    };
    options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| protocol::PermissionOutcome::Selected {
            option_id: option.option_id.clone(),
        })
        .unwrap_or(protocol::PermissionOutcome::Cancelled)
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
