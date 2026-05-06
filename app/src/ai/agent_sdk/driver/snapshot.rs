use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::server::server_api::ai::{AIClient, InitialSnapshotToken};
use warpui::r#async::executor::Background;

pub(super) const DEFAULT_DECLARATIONS_SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const DEFAULT_SNAPSHOT_UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) struct DeclarationsWriterHandle;

impl DeclarationsWriterHandle {
    pub(crate) fn new(
        _task_id: AmbientAgentTaskId,
        _working_dir: PathBuf,
        _background: &Background,
    ) -> Self {
        Self
    }

    pub(crate) fn append(&self, _paths: Vec<PathBuf>) {}
}

pub(crate) async fn upload_snapshot_for_handoff(
    _repo_paths: Vec<PathBuf>,
    _orphan_files: Vec<PathBuf>,
    _ai_client: Arc<dyn AIClient>,
    _http_client: &http_client::Client,
) -> anyhow::Result<Option<InitialSnapshotToken>> {
    Ok(None)
}
