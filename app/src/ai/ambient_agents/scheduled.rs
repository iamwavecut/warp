use serde::{Deserialize, Serialize};

use super::AgentConfigSnapshot;

use crate::{
    cloud_object::{
        model::{
            generic_string_model::{GenericStringModel, GenericStringObjectId, StringModel},
            json_model::{JsonModel, JsonSerializer},
        },
        GenericCloudObject, GenericStringObjectFormat, GenericStringObjectUniqueKey,
        JsonObjectType, Revision, ServerCloudObject,
    },
    server::sync_queue::QueueItem,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
/// A ScheduledAmbientAgent represents configuration for ambient agents that run on a cron schedule.
pub struct ScheduledAmbientAgent {
    /// Agent name
    #[serde(default)]
    pub name: String,
    /// Cron schedule expression
    #[serde(default)]
    pub cron_schedule: String,
    /// Whether the scheduled agent is enabled
    #[serde(default)]
    pub enabled: bool,
    /// The prompt to use for the scheduled agent
    #[serde(default)]
    pub prompt: String,
    /// The latest failure to execute this scheduled agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_spawn_error: Option<String>,
    /// Configuration for how the ambient agent should run.
    #[serde(default, skip_serializing_if = "AgentConfigSnapshot::is_empty")]
    pub agent_config: AgentConfigSnapshot,
}

pub type CloudScheduledAmbientAgent =
    GenericCloudObject<GenericStringObjectId, CloudScheduledAmbientAgentModel>;
pub type CloudScheduledAmbientAgentModel =
    GenericStringModel<ScheduledAmbientAgent, JsonSerializer>;

impl ScheduledAmbientAgent {
    pub fn new(name: String, cron_schedule: String, enabled: bool, prompt: String) -> Self {
        Self {
            name,
            cron_schedule,
            enabled,
            prompt,
            last_spawn_error: None,
            agent_config: Default::default(),
        }
    }
}

impl StringModel for ScheduledAmbientAgent {
    type CloudObjectType = CloudScheduledAmbientAgent;

    fn model_type_name(&self) -> &'static str {
        "Scheduled ambient agent"
    }

    fn should_enforce_revisions() -> bool {
        true
    }

    fn model_format() -> GenericStringObjectFormat {
        GenericStringObjectFormat::Json(JsonObjectType::ScheduledAmbientAgent)
    }

    fn display_name(&self) -> String {
        self.name.clone()
    }

    fn update_object_queue_item(
        &self,
        revision_ts: Option<Revision>,
        object: &CloudScheduledAmbientAgent,
    ) -> QueueItem {
        QueueItem::UpdateScheduledAmbientAgent {
            model: object.model().clone().into(),
            id: object.id,
            revision: revision_ts.or_else(|| object.metadata.revision.clone()),
        }
    }

    fn uniqueness_key(&self) -> Option<GenericStringObjectUniqueKey> {
        None
    }

    fn new_from_server_update(&self, server_cloud_object: &ServerCloudObject) -> Option<Self> {
        if let ServerCloudObject::ScheduledAmbientAgent(server_scheduled_agent) =
            server_cloud_object
        {
            return Some(server_scheduled_agent.model.clone().string_model);
        }
        None
    }

    fn should_show_activity_toasts() -> bool {
        false
    }

    fn warn_if_unsaved_at_quit() -> bool {
        true
    }
}

impl JsonModel for ScheduledAmbientAgent {
    fn json_object_type() -> JsonObjectType {
        JsonObjectType::ScheduledAmbientAgent
    }
}
