use std::path::PathBuf;
use std::time::Duration;

use crate::ai::ambient_agents::AmbientAgentTaskId;
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
