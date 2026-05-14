use crate::{
    cloud_object::{ServerCloudObject, ServerMetadata, ServerPermissions},
    server::{ids::ServerId, server_api::object::ObjectClient},
    workspaces::user_profiles::UserProfileWithUID,
};

use std::sync::Arc;
use warpui::{Entity, ModelContext, SingletonEntity};

pub enum ListenerEvent {}

/// Local-first builds do not subscribe to Warp Drive websocket updates.
///
/// The message enum is kept because UpdateManager still accepts imported or
/// test-provided object update messages, but startup no longer creates any
/// remote subscription or retry loop.
pub struct Listener {}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum ObjectUpdateMessage {
    ObjectMetadataChanged {
        metadata: ServerMetadata,
    },
    ObjectPermissionsChangedV2 {
        object_uid: ServerId,
        permissions: ServerPermissions,
        user_profiles: Vec<UserProfileWithUID>,
    },
    ObjectContentChanged {
        server_object: Box<ServerCloudObject>,
        last_editor: Option<UserProfileWithUID>,
    },
}

impl Listener {
    pub fn new(
        _cloud_objects_client: Arc<dyn ObjectClient>,
        _ctx: &mut ModelContext<Self>,
    ) -> Self {
        Self {}
    }

    #[cfg(test)]
    pub fn mock(ctx: &mut ModelContext<Self>) -> Self {
        use crate::server::server_api::ServerApiProvider;

        Self::new(ServerApiProvider::new_for_test().get(), ctx)
    }
}

impl Entity for Listener {
    type Event = ListenerEvent;
}

impl SingletonEntity for Listener {}
