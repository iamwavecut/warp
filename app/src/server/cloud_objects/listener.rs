use crate::server::server_api::object::ObjectClient;

use std::sync::Arc;
use warpui::{Entity, ModelContext, SingletonEntity};

pub enum ListenerEvent {}

/// Local-first builds do not subscribe to Warp Drive websocket updates.
pub struct Listener {}

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
