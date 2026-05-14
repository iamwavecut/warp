//! Settings for local agent workspace functionality.
//!
//! This module contains user-specific settings for local agent workspace
//! features, such as remembering the last selected environment.

use settings::{macros::define_settings_group, SupportedPlatforms, SyncToCloud};

use crate::server::ids::SyncId;

define_settings_group!(CloudAgentSettings, settings: [
    last_selected_environment_id: LastSelectedEnvironmentId {
        type: Option<SyncId>,
        default: None,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: true,
    }
]);
