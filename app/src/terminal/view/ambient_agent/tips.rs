//! Tips for the local agent loading screen.

use crate::ai::agent_tips::AITip;
use warpui::keymap::Keystroke;
use warpui::AppContext;

/// A local agent loading tip.
#[derive(Clone, Debug)]
pub struct AgentLoadingTip {
    text: String,
}

impl AgentLoadingTip {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl AITip for AgentLoadingTip {
    fn keystroke(&self, _app: &AppContext) -> Option<Keystroke> {
        None
    }

    fn link(&self) -> Option<String> {
        None
    }

    fn description(&self) -> &str {
        &self.text
    }
}

/// Returns tips for the local agent loading screen.
pub fn get_agent_loading_tips() -> Vec<AgentLoadingTip> {
    vec![
        AgentLoadingTip::new("Use MCP servers to give local agents access to local tools."),
        AgentLoadingTip::new("Keep provider keys in secure storage or environment variables."),
        AgentLoadingTip::new("Attach terminal blocks as context when a command result matters."),
        AgentLoadingTip::new("Use project rules to keep local agents aligned with this repo."),
        AgentLoadingTip::new("Fork an agent conversation when you want to try another path."),
        AgentLoadingTip::new(
            "Use agent profiles to separate fast local models from larger reasoning models.",
        ),
        AgentLoadingTip::new(
            "Add files and directories as context before asking for code changes.",
        ),
        AgentLoadingTip::new("Use local MCP tools for project-specific automation."),
        AgentLoadingTip::new("Store reusable prompts as skills when a workflow repeats often."),
        AgentLoadingTip::new("Review tool permissions before letting an agent modify files."),
    ]
}
