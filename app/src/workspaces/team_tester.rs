use warpui::{Entity, ModelContext, SingletonEntity};

#[derive(Clone)]
pub struct TeamTesterStatus {}

impl TeamTesterStatus {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {}
    }

    #[cfg(test)]
    pub fn mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(ctx)
    }
}

impl Entity for TeamTesterStatus {
    type Event = ();
}

impl SingletonEntity for TeamTesterStatus {}
