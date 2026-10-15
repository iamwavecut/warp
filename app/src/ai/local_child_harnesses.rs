use warp_cli::agent::Harness;
use warp_core::features::FeatureFlag;

pub(crate) fn local_child_harness_disabled_message(harness: Harness) -> Option<&'static str> {
    if harness == Harness::Pi {
        return Some("Pi child panes are unsupported. Use the local ACP driver.");
    }
    if FeatureFlag::LocalClaudeCodexChildHarnesses.is_enabled() {
        return None;
    }

    match harness {
        Harness::Claude => Some("Local Claude Code child agents are temporarily disabled."),
        Harness::Codex => Some("Local Codex child agents are temporarily disabled."),
        Harness::Oz | Harness::OpenCode | Harness::Gemini | Harness::Pi | Harness::Unknown => None,
    }
}

pub(crate) fn local_child_harness_is_enabled(harness: Harness) -> bool {
    local_child_harness_disabled_message(harness).is_none()
}

#[cfg(test)]
#[path = "local_child_harnesses_tests.rs"]
mod tests;
