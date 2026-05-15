mod block;
mod footer;
mod harness_selector;
mod loading_screen;
mod model;
mod model_selector;
mod progress;
mod progress_ui_state;
mod tips;
mod view_impl;

pub use block::*;
pub use footer::{render_error_footer, render_loading_footer};
pub use harness_selector::{
    HarnessSelector, HarnessSelectorAction, HarnessSelectorEvent, NakedHeaderButtonTheme,
};
pub use loading_screen::{render_cloud_mode_error_screen, render_cloud_mode_loading_screen};
pub use model::{AgentProgress, AmbientAgentViewModel, AmbientAgentViewModelEvent, Status};
pub use model_selector::{ModelSelector, ModelSelectorAction, ModelSelectorEvent};
pub use progress::{render_progress, ProgressProps, ProgressStep, ProgressStepState};
pub use progress_ui_state::AmbientAgentProgressUIState;
pub use tips::{get_agent_loading_tips, AgentLoadingTip};

use warp_core::features::FeatureFlag;
use warpui::{AppContext, ModelHandle};

use crate::ai::blocklist::agent_view::{AgentViewController, AgentViewState};
use crate::terminal::TerminalModel;

/// Returns `true` when an agent shared session is in any pre-first-exchange phase —
/// either still spawning (loading screen) or running setup commands before the first
/// agent turn. In this state, we hide the interactive input and render a loading footer.
pub fn is_cloud_agent_pre_first_exchange(
    ambient_agent_view_model: Option<&ModelHandle<AmbientAgentViewModel>>,
    agent_view_controller: &ModelHandle<AgentViewController>,
    terminal_model: &TerminalModel,
    app: &AppContext,
) -> bool {
    if !FeatureFlag::AgentView.is_enabled() {
        return false;
    }

    let Some(ambient_agent_view_model) = ambient_agent_view_model else {
        return false;
    };

    let view_model = ambient_agent_view_model.as_ref(app);

    let is_in_pre_first_exchange_status = matches!(
        view_model.status(),
        Status::WaitingForSession { .. } | Status::AgentRunning
    );
    if !is_in_pre_first_exchange_status {
        return false;
    }

    let agent_view_state = agent_view_controller.as_ref(app).agent_view_state().clone();
    let AgentViewState::Active { origin, .. } = agent_view_state else {
        return false;
    };

    if !origin.is_cloud_agent() {
        return false;
    }

    // For non-oz harness runs, there is no Oz `AppendedExchange` to key off of, so we also
    // exit the pre-first-exchange phase when the harness CLI (e.g. `claude`, `gemini`) has
    // been detected. See `mark_harness_command_started`.
    if view_model.harness_command_started() {
        return false;
    }

    // Loading phase (`WaitingForSession`): no setup commands have started yet, but we're
    // still pre-first-exchange. Skip the block-list flag check.
    if matches!(view_model.status(), Status::WaitingForSession { .. }) {
        return true;
    }

    terminal_model
        .block_list()
        .is_executing_oz_environment_startup_commands()
}
