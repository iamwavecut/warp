use warpui::{Entity, ModelContext, SingletonEntity};

pub struct SessionPermissionsManager {}

impl SessionPermissionsManager {
    pub(crate) fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {}
    }
}

impl Entity for SessionPermissionsManager {
    type Event = ();
}

impl SingletonEntity for SessionPermissionsManager {}
