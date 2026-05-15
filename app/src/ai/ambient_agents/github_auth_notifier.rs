//! Local placeholder for hosted GitHub authentication coordination.

use warpui::{Entity, SingletonEntity};

/// Singleton notifier for GitHub authentication state.
///
pub struct GitHubAuthNotifier;

impl GitHubAuthNotifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitHubAuthNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Entity for GitHubAuthNotifier {
    type Event = ();
}

impl SingletonEntity for GitHubAuthNotifier {}
