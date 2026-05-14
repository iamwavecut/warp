use std::sync::Arc;

use anyhow::Result;
use channel_versions::Changelog;

use crate::server::server_api::ServerApi;

pub async fn get_current_changelog(server_api: Arc<ServerApi>) -> Result<Option<Changelog>> {
    let _ = server_api;
    Ok(None)
}
