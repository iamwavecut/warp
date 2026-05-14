//! Logic to convert from / to [`ServerExperiment`].

use std::fmt::{Display, Formatter};

use super::ServerExperiment;
use anyhow::{Ok, Result};

impl Display for ServerExperiment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Self::SessionSharingControl => "SESSION_SHARING_CONTROL",
            Self::SessionSharingExperiment => "SESSION_SHARING_EXPERIMENT",
            Self::DisableAgentModeExperiment => "DISABLE_AGENT_MODE_EXPERIMENT",
            Self::EnvVarsEarlyAccessExperiment => "ENV_VARS_EARLY_ACCESS_EXPERIMENT",
            Self::WindowsLaunchExperiment => "WINDOWS_LAUNCH_EXPERIMENT",
            Self::TmuxSshWarpificationControl => "TMUX_SSH_WARPIFICATION_CONTROL",
            Self::TmuxSshWarpificationExperiment => "TMUX_SSH_WARPIFICATION_EXPERIMENT",
            Self::CodebaseContextControl => "CODEBASE_CONTEXT_CONTROL",
            Self::CodebaseContextExperiment => "CODEBASE_CONTEXT_EXPERIMENT",
            Self::SuggestedCodeDiffsControl => "SUGGESTED_CODE_DIFFS_CONTROL",
            Self::SuggestedCodeDiffsExperiment => "SUGGESTED_CODE_DIFFS_EXPERIMENT",
            Self::BuildPlanAutoReloadControl => "BUILD_PLAN_AUTO_RELOAD_CONTROL",
            Self::BuildPlanAutoReloadBannerToggle => "BUILD_PLAN_AUTO_RELOAD_BANNER_TOGGLE",
            Self::BuildPlanAutoReloadPostPurchaseModal => {
                "BUILD_PLAN_AUTO_RELOAD_POST_PURCHASE_MODAL"
            }
            #[cfg(test)]
            Self::TestExperiment => "TEST_EXPERIMENT",
        };
        write!(f, "{str}")
    }
}

impl ServerExperiment {
    pub fn from_string(s: String) -> Result<Self> {
        match s.as_str() {
            "SESSION_SHARING_CONTROL" => Ok(Self::SessionSharingControl),
            "SESSION_SHARING_EXPERIMENT" => Ok(Self::SessionSharingExperiment),
            "DISABLE_AGENT_MODE_EXPERIMENT" => Ok(Self::DisableAgentModeExperiment),
            "ENV_VARS_EARLY_ACCESS_EXPERIMENT" => Ok(Self::EnvVarsEarlyAccessExperiment),
            "WINDOWS_LAUNCH_EXPERIMENT" => Ok(Self::WindowsLaunchExperiment),
            "TMUX_SSH_WARPIFICATION_CONTROL" => Ok(Self::TmuxSshWarpificationControl),
            "TMUX_SSH_WARPIFICATION_EXPERIMENT" => Ok(Self::TmuxSshWarpificationExperiment),
            "CODEBASE_CONTEXT_EXPERIMENT" => Ok(Self::CodebaseContextExperiment),
            "CODEBASE_CONTEXT_CONTROL" => Ok(Self::CodebaseContextControl),
            "SUGGESTED_CODE_DIFFS_CONTROL" => Ok(Self::SuggestedCodeDiffsControl),
            "SUGGESTED_CODE_DIFFS_EXPERIMENT" => Ok(Self::SuggestedCodeDiffsExperiment),
            "BUILD_PLAN_AUTO_RELOAD_CONTROL" => Ok(Self::BuildPlanAutoReloadControl),
            "BUILD_PLAN_AUTO_RELOAD_BANNER_TOGGLE" => Ok(Self::BuildPlanAutoReloadBannerToggle),
            "BUILD_PLAN_AUTO_RELOAD_POST_PURCHASE_MODAL" => {
                Ok(Self::BuildPlanAutoReloadPostPurchaseModal)
            }
            s => Err(anyhow::anyhow!(
                "String doesn't match any server experiment variant {s}"
            )),
        }
    }
}
