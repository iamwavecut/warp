use crate::{
    cloud_object::{
        model::{
            actions::{ObjectActionHistory, ObjectActionType},
            generic_string_model::GenericStringObjectId,
        },
        BulkCreateCloudObjectResult, BulkCreateGenericStringObjectsRequest,
        CreateCloudObjectResult, CreateObjectRequest, GenericStringObjectFormat,
        GenericStringObjectUniqueKey, ObjectDeleteResult, ObjectMetadataUpdateResult,
        ObjectPermissionUpdateResult, ObjectPermissionsUpdateData, ObjectType, ObjectsToUpdate,
        Owner, Revision, ServerFolder, ServerMetadata, ServerNotebook, ServerObject,
        ServerPermissions, ServerWorkflow, UpdateCloudObjectResult,
    },
    drive::{folders::FolderId, sharing::SharingAccessLevel},
    notebooks::NotebookId,
    server::{
        cloud_objects::update_manager::{GetCloudObjectResponse, InitialLoadResponse},
        ids::ServerId,
        server_api::ServerApi,
        sync_queue::SerializedModel,
    },
    workflows::WorkflowId,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(test)]
use mockall::{automock, predicate::*};
use std::collections::HashMap;
use warp_graphql::object_permissions::AccessLevel;

/// Identifies a guest to remove from an object.
#[derive(Clone, Debug)]
pub enum GuestIdentifier {
    /// Remove a user guest by their email address.
    Email(String),
    /// Remove a team guest by their team UID.
    TeamUid(ServerId),
}

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait ObjectClient: 'static + Send + Sync {
    /// This method saves a workflow for a given owner and returns it on success.
    async fn create_workflow(
        &self,
        request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult>;

    /// Updates a workflow with the new data. The update may be rejected if a revision
    /// is specified _and_ that revision is not the current revision of the object in storage.
    async fn update_workflow(
        &self,
        workflow_id: WorkflowId,
        data: SerializedModel,
        revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<ServerWorkflow>>;

    /// Creates n generic string objects in a single graphql request. Use
    /// this rather than calling create_generic_string_object multiple times
    /// in a loop.
    async fn bulk_create_generic_string_objects(
        &self,
        owner: Owner,
        objects: &[BulkCreateGenericStringObjectsRequest],
    ) -> Result<BulkCreateCloudObjectResult>;

    async fn create_generic_string_object(
        &self,
        format: GenericStringObjectFormat,
        uniqueness_key: Option<GenericStringObjectUniqueKey>,
        request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult>;

    /// Creates a notebook on the server, returning the ID and revision of the object after
    /// creation.
    async fn create_notebook(
        &self,
        request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult>;

    /// Updates a notebook with the new title and data. The update may be rejected if a revision
    /// is specified _and_ that revision is not the current revision of the object in storage.
    async fn update_notebook(
        &self,
        notebook_id: NotebookId,
        title: Option<String>,
        data: Option<SerializedModel>,
        revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<ServerNotebook>>;

    async fn create_folder(&self, request: CreateObjectRequest) -> Result<CreateCloudObjectResult>;

    async fn update_folder(
        &self,
        folder_id: FolderId,
        name: SerializedModel,
    ) -> Result<UpdateCloudObjectResult<ServerFolder>>;

    async fn update_generic_string_object(
        &self,
        object_id: GenericStringObjectId,
        model: SerializedModel,
        revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<Box<dyn ServerObject>>>;

    /// Sets the current editor of the notebook to be the logged in user
    async fn grab_notebook_edit_access(&self, notebook_id: NotebookId) -> Result<ServerMetadata>;
    /// Sets the current editor of the notebook to be null
    async fn give_up_notebook_edit_access(&self, notebook_id: NotebookId)
        -> Result<ServerMetadata>;

    async fn fetch_changed_objects(
        &self,
        objects_to_update: ObjectsToUpdate,
        force_refresh: bool,
    ) -> Result<InitialLoadResponse>;

    async fn fetch_single_cloud_object(&self, id: ServerId) -> Result<GetCloudObjectResponse>;

    // Transfers a notebook to the given owner
    async fn transfer_notebook_owner(&self, notebook_id: NotebookId, owner: Owner) -> Result<bool>;

    async fn transfer_workflow_owner(&self, workflow_id: WorkflowId, owner: Owner) -> Result<bool>;

    async fn transfer_generic_string_object_owner(
        &self,
        workflow_id: GenericStringObjectId,
        owner: Owner,
    ) -> Result<bool>;

    async fn trash_object(&self, id: ServerId) -> Result<bool>;

    async fn untrash_object(&self, id: ServerId) -> Result<ObjectMetadataUpdateResult>;

    async fn delete_object(&self, id: ServerId) -> Result<ObjectDeleteResult>;

    async fn empty_trash(&self, owner: Owner) -> Result<ObjectDeleteResult>;

    async fn move_object(
        &self,
        id: ServerId,
        folder_id: Option<FolderId>,
        owner: Owner,
        object_type: ObjectType,
    ) -> Result<bool>;

    async fn record_object_action(
        &self,
        id: ServerId,
        action_type: ObjectActionType,
        timestamp: DateTime<Utc>,
        data: Option<String>,
    ) -> Result<ObjectActionHistory>;

    async fn leave_object(&self, id: ServerId) -> Result<ObjectDeleteResult>;

    async fn set_object_link_permissions(
        &self,
        object_id: ServerId,
        access_level: SharingAccessLevel,
    ) -> Result<ObjectPermissionUpdateResult>;

    async fn remove_object_link_permissions(
        &self,
        object_id: ServerId,
    ) -> Result<ObjectPermissionUpdateResult>;

    async fn add_object_guests(
        &self,
        object_id: ServerId,
        guest_emails: Vec<String>,
        access_level: AccessLevel,
    ) -> Result<ObjectPermissionsUpdateData>;

    async fn update_object_guests(
        &self,
        object_id: ServerId,
        guest_emails: Vec<String>,
        access_level: AccessLevel,
    ) -> Result<ServerPermissions>;

    async fn remove_object_guest(
        &self,
        object_id: ServerId,
        guest: GuestIdentifier,
    ) -> Result<ServerPermissions>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl ObjectClient for ServerApi {
    async fn create_workflow(
        &self,
        _request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        Err(Self::backend_disabled_error())
    }

    async fn update_workflow(
        &self,
        _workflow_id: WorkflowId,
        _data: SerializedModel,
        _revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<ServerWorkflow>> {
        Err(Self::backend_disabled_error())
    }

    async fn bulk_create_generic_string_objects(
        &self,
        _owner: Owner,
        _objects: &[BulkCreateGenericStringObjectsRequest],
    ) -> Result<BulkCreateCloudObjectResult> {
        Err(Self::backend_disabled_error())
    }

    async fn create_generic_string_object(
        &self,
        _format: GenericStringObjectFormat,
        _uniqueness_key: Option<GenericStringObjectUniqueKey>,
        _request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        Err(Self::backend_disabled_error())
    }

    async fn create_notebook(
        &self,
        _request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        Err(Self::backend_disabled_error())
    }

    async fn update_notebook(
        &self,
        _notebook_id: NotebookId,
        _title: Option<String>,
        _data: Option<SerializedModel>,
        _revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<ServerNotebook>> {
        Err(Self::backend_disabled_error())
    }

    async fn create_folder(
        &self,
        _request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        Err(Self::backend_disabled_error())
    }

    async fn update_folder(
        &self,
        _folder_id: FolderId,
        _name: SerializedModel,
    ) -> Result<UpdateCloudObjectResult<ServerFolder>> {
        Err(Self::backend_disabled_error())
    }

    async fn update_generic_string_object(
        &self,
        _object_id: GenericStringObjectId,
        _model: SerializedModel,
        _revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<Box<dyn ServerObject>>> {
        Err(Self::backend_disabled_error())
    }

    async fn grab_notebook_edit_access(&self, _notebook_id: NotebookId) -> Result<ServerMetadata> {
        Err(Self::backend_disabled_error())
    }

    async fn give_up_notebook_edit_access(
        &self,
        _notebook_id: NotebookId,
    ) -> Result<ServerMetadata> {
        Err(Self::backend_disabled_error())
    }

    async fn fetch_changed_objects(
        &self,
        _objects_to_update: ObjectsToUpdate,
        _force_refresh: bool,
    ) -> Result<InitialLoadResponse> {
        Ok(InitialLoadResponse::default())
    }

    async fn fetch_single_cloud_object(&self, _id: ServerId) -> Result<GetCloudObjectResponse> {
        Err(Self::backend_disabled_error())
    }

    async fn transfer_notebook_owner(
        &self,
        _notebook_id: NotebookId,
        _owner: Owner,
    ) -> Result<bool> {
        Err(Self::backend_disabled_error())
    }

    async fn transfer_workflow_owner(
        &self,
        _workflow_id: WorkflowId,
        _owner: Owner,
    ) -> Result<bool> {
        Err(Self::backend_disabled_error())
    }

    async fn transfer_generic_string_object_owner(
        &self,
        _workflow_id: GenericStringObjectId,
        _owner: Owner,
    ) -> Result<bool> {
        Err(Self::backend_disabled_error())
    }

    async fn trash_object(&self, _id: ServerId) -> Result<bool> {
        Err(Self::backend_disabled_error())
    }

    async fn untrash_object(&self, _id: ServerId) -> Result<ObjectMetadataUpdateResult> {
        Err(Self::backend_disabled_error())
    }

    async fn delete_object(&self, _id: ServerId) -> Result<ObjectDeleteResult> {
        Err(Self::backend_disabled_error())
    }

    async fn empty_trash(&self, _owner: Owner) -> Result<ObjectDeleteResult> {
        Err(Self::backend_disabled_error())
    }

    async fn move_object(
        &self,
        _id: ServerId,
        _folder_id: Option<FolderId>,
        _owner: Owner,
        _object_type: ObjectType,
    ) -> Result<bool> {
        Err(Self::backend_disabled_error())
    }

    async fn record_object_action(
        &self,
        _id: ServerId,
        _action_type: ObjectActionType,
        _timestamp: DateTime<Utc>,
        _data: Option<String>,
    ) -> Result<ObjectActionHistory> {
        Err(Self::backend_disabled_error())
    }

    async fn leave_object(&self, _id: ServerId) -> Result<ObjectDeleteResult> {
        Err(Self::backend_disabled_error())
    }

    async fn set_object_link_permissions(
        &self,
        _object_id: ServerId,
        _access_level: SharingAccessLevel,
    ) -> Result<ObjectPermissionUpdateResult> {
        Err(Self::backend_disabled_error())
    }

    async fn remove_object_link_permissions(
        &self,
        _object_id: ServerId,
    ) -> Result<ObjectPermissionUpdateResult> {
        Err(Self::backend_disabled_error())
    }

    async fn add_object_guests(
        &self,
        _object_id: ServerId,
        _guest_emails: Vec<String>,
        _access_level: AccessLevel,
    ) -> Result<ObjectPermissionsUpdateData> {
        Err(Self::backend_disabled_error())
    }

    async fn update_object_guests(
        &self,
        _object_id: ServerId,
        _guest_emails: Vec<String>,
        _access_level: AccessLevel,
    ) -> Result<ServerPermissions> {
        Err(Self::backend_disabled_error())
    }

    async fn remove_object_guest(
        &self,
        _object_id: ServerId,
        _guest: GuestIdentifier,
    ) -> Result<ServerPermissions> {
        Err(Self::backend_disabled_error())
    }
}
