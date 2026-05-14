use std::{env, fs::read_to_string, sync::Arc};

use anyhow::{Context as _, Result};
use channel_versions::ChannelVersions;

use crate::server::server_api::ServerApi;

// Load channel versions from a local file for tests. Runtime autoupdate checks are disabled in
// this local-first fork so the app never contacts Warp release services.
pub async fn fetch_channel_versions(
    nonce: &str,
    server_api: Arc<ServerApi>,
    include_changelogs: bool,
    is_daily: bool,
) -> Result<ChannelVersions> {
    let _ = (nonce, server_api, include_changelogs, is_daily);

    if let Ok(path) = env::var("WARP_CHANNEL_VERSIONS_PATH") {
        // Load channel versions from local filesystem. Used for testing both
        // autoupdate and changelog behavior.
        let path = shellexpand::tilde(&path);
        let channel_versions_string = read_to_string::<&str>(&path)?;
        return serde_json::from_str(channel_versions_string.as_str())
            .context("Failed to parse channel versions JSON");
    }

    anyhow::bail!("Autoupdate version checks are disabled in the local-first build")
}
