use chrono::{DateTime, Utc};
use derivative::Derivative;
use std::sync::Arc;
use uuid::Uuid;
use warp_graphql::scalars::time::ServerTimestamp;
use warpui::{r#async::FutureId, Entity, ModelContext, SingletonEntity};

use super::{
    ids::{ClientId, ObjectUid, ServerId, SyncId},
    server_api::object::ObjectClient,
};

use crate::server::cloud_objects::update_manager::InitiatedBy;
use crate::{
    ai::facts::CloudAIFactModel,
    cloud_object::{
        model::actions::{ObjectActionHistory, ObjectActionType},
        CloudObjectEventEntrypoint, GenericStringObjectFormat, GenericStringObjectUniqueKey,
        ObjectType, Owner, Revision, RevisionAndLastEditor, ServerCloudObject, ServerCreationInfo,
    },
    drive::{folders::CloudFolderModel, CloudObjectTypeAndId},
    env_vars::CloudEnvVarCollectionModel,
    notebooks::CloudNotebookModel,
    settings::cloud_preferences::CloudPreferenceModel,
    workflows::{workflow_enum::CloudWorkflowEnumModel, CloudWorkflowModel},
};

// A newtype for a serialized model that wraps a plain string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedModel(String);

impl SerializedModel {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn model_as_str(&self) -> &str {
        &self.0
    }

    pub fn take(self) -> String {
        self.0
    }
}

impl From<String> for SerializedModel {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct GenericStringObjectToCreate {
    pub id: ClientId,
    pub format: GenericStringObjectFormat,
    pub serialized_model: Arc<SerializedModel>,
    pub initial_folder_id: Option<SyncId>,
    pub entrypoint: CloudObjectEventEntrypoint,
    pub uniqueness_key: Option<GenericStringObjectUniqueKey>,
    pub initiated_by: InitiatedBy,
}

/// An ID for a `QueueItem` in the sync queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueueItemId(Uuid);

impl QueueItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Local object mutation shape retained for callers that still build cloud-object updates.
#[derive(Derivative, Debug)]
#[derivative(PartialEq, Eq, Clone)]
pub enum QueueItem {
    CreateObject {
        object_type: ObjectType,
        owner: Owner,
        id: ClientId,
        title: Option<Arc<String>>,
        serialized_model: Option<Arc<SerializedModel>>,
        initial_folder_id: Option<SyncId>,
        entrypoint: CloudObjectEventEntrypoint,
        initiated_by: InitiatedBy,
    },
    CreateWorkflow {
        object_type: ObjectType,
        owner: Owner,
        id: ClientId,
        #[derivative(PartialEq = "ignore")]
        model: Arc<CloudWorkflowModel>,
        initial_folder_id: Option<SyncId>,
        entrypoint: CloudObjectEventEntrypoint,
        initiated_by: InitiatedBy,
    },
    BulkCreateGenericStringObjects {
        owner: Owner,
        objects: Vec<GenericStringObjectToCreate>,
    },
    UpdateNotebook {
        model: Arc<CloudNotebookModel>,
        id: SyncId,
        revision: Option<Revision>,
    },
    UpdateWorkflow {
        model: Arc<CloudWorkflowModel>,
        id: SyncId,
        revision: Option<Revision>,
    },
    UpdateFolder {
        id: SyncId,
        model: Arc<CloudFolderModel>,
    },
    UpdateCloudPreferences {
        model: Arc<CloudPreferenceModel>,
        id: SyncId,
        revision: Option<Revision>,
    },
    UpdateEnvVarCollection {
        model: Arc<CloudEnvVarCollectionModel>,
        id: SyncId,
        revision: Option<Revision>,
    },
    UpdateWorkflowEnum {
        model: Arc<CloudWorkflowEnumModel>,
        id: SyncId,
        revision: Option<Revision>,
    },
    UpdateAIFact {
        model: Arc<CloudAIFactModel>,
        id: SyncId,
        revision: Option<Revision>,
    },
    RecordObjectAction {
        id_and_type: CloudObjectTypeAndId,
        action_type: ObjectActionType,
        action_timestamp: DateTime<Utc>,
        data: Option<String>,
    },
}

#[derive(Derivative, Clone, Debug)]
#[derivative(PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum CreationFailureReason {
    UniqueKeyConflict {
        id: String,
        initiated_by: InitiatedBy,
    },
    Denied {
        message: String,
        client_id: ClientId,
        initiated_by: InitiatedBy,
    },
    Other {
        id: String,
        initiated_by: InitiatedBy,
    },
}

#[derive(Derivative, Clone, Debug)]
#[derivative(PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
#[allow(clippy::large_enum_variant)]
pub enum SyncQueueEvent {
    ObjectCreationSuccessful {
        server_creation_info: ServerCreationInfo,
        client_id: ClientId,
        revision_and_editor: RevisionAndLastEditor,
        metadata_ts: ServerTimestamp,
        initiated_by: InitiatedBy,
    },
    ObjectUpdateSuccessful {
        server_id: ServerId,
        revision_and_editor: RevisionAndLastEditor,
    },
    ObjectUpdateRejected {
        id: String,
        #[derivative(PartialEq = "ignore")]
        object: Arc<ServerCloudObject>,
    },
    #[allow(dead_code)]
    ObjectUpdateFeatureNotAvailable {
        id: String,
    },
    ObjectCreationFailure {
        reason: CreationFailureReason,
    },
    ObjectUpdateFailure {
        id: SyncId,
    },
    ReportObjectActionFailed {
        uid: ObjectUid,
        action_timestamp: DateTime<Utc>,
    },
    ReportObjectActionSucceeded {
        uid: ObjectUid,
        action_timestamp: DateTime<Utc>,
        action_history: ObjectActionHistory,
    },
}

/// Local-first compatibility facade for legacy cloud-object callers.
///
/// The hosted implementation used to dequeue mutations to Warp's object backend. In this fork,
/// objects are persisted locally through `CloudModel`, so enqueueing a `QueueItem` intentionally
/// does not spawn network work or retain a remote-sync backlog.
pub struct SyncQueue {
    queue: Vec<(QueueItemId, QueueItem)>,
    spawned_futures: Vec<FutureId>,
}

impl SyncQueue {
    #[cfg(test)]
    pub fn mock(ctx: &mut ModelContext<Self>) -> Self {
        use super::server_api::ServerApiProvider;

        Self::new(
            Default::default(),
            ServerApiProvider::new_for_test().get(),
            ctx,
        )
    }

    pub fn new(
        queue_items: Vec<QueueItem>,
        object_client: Arc<dyn ObjectClient>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let _ = (queue_items, object_client, ctx);
        Self {
            queue: Vec::new(),
            spawned_futures: Vec::new(),
        }
    }

    pub fn is_dequeueing(&self) -> bool {
        false
    }

    pub fn stop_dequeueing(&mut self) {}

    pub fn start_dequeueing(&mut self, ctx: &mut ModelContext<Self>) {
        let _ = ctx;
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.spawned_futures.clear();
    }

    pub fn enqueue(&mut self, item: QueueItem, ctx: &mut ModelContext<Self>) -> QueueItemId {
        let _ = (item, ctx);
        QueueItemId::new()
    }

    #[cfg(test)]
    pub fn queue(&self) -> &Vec<(QueueItemId, QueueItem)> {
        &self.queue
    }

    #[cfg(test)]
    pub fn spawned_futures(&self) -> &Vec<FutureId> {
        &self.spawned_futures
    }
}

impl Entity for SyncQueue {
    type Event = SyncQueueEvent;
}

impl SingletonEntity for SyncQueue {}
