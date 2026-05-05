use crate::ai::ambient_agents::AmbientAgentTaskId;
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

pub struct TaskStatusSyncModel;

pub enum TaskStatusSyncModelEvent {}

impl TaskStatusSyncModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self
    }

    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub fn register_cli_session(
        &mut self,
        _terminal_view_id: EntityId,
        _task_id: AmbientAgentTaskId,
        _ctx: &mut ModelContext<Self>,
    ) {
    }
}

impl Entity for TaskStatusSyncModel {
    type Event = TaskStatusSyncModelEvent;
}

impl SingletonEntity for TaskStatusSyncModel {}
