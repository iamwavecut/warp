use super::user_workspaces::UserWorkspaces;
use super::workspace::WorkspaceUid;
use crate::persistence::ModelEvent;
use crate::report_if_error;
use anyhow::Context;
use futures::channel::oneshot::{self, Receiver};
use std::sync::mpsc::SyncSender;
use warpui::{Entity, ModelContext, SingletonEntity};

/// TeamUpdateManager keeps the local current-workspace selection persisted.
/// Hosted team management and workspace metadata polling are intentionally absent in this fork.
pub struct TeamUpdateManager {
    model_event_sender: Option<SyncSender<ModelEvent>>,
}

impl TeamUpdateManager {
    pub fn new(
        model_event_sender: Option<SyncSender<ModelEvent>>,
        _ctx: &mut ModelContext<Self>,
    ) -> Self {
        Self { model_event_sender }
    }

    #[cfg(test)]
    pub fn mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(Default::default(), ctx)
    }

    pub fn stop_polling_for_workspace_metadata_updates(&mut self) {}

    /// Hosted workspace metadata refresh is disabled. The receiver resolves immediately so
    /// callers that still use the old refresh hook do not schedule network work.
    pub fn refresh_workspace_metadata(&mut self, _ctx: &mut ModelContext<Self>) -> Receiver<()> {
        let (tx, rx) = oneshot::channel::<()>();
        let _ = tx.send(());
        rx
    }

    fn save_to_db(&self, events: impl IntoIterator<Item = ModelEvent>) {
        let model_event_sender = self.model_event_sender.clone();
        if let Some(model_event_sender) = &model_event_sender {
            for event in events {
                report_if_error!(model_event_sender
                    .send(event)
                    .context("Unable to save teams metadata to sqlite"));
            }
        }
    }

    pub fn set_current_workspace_uid(
        &mut self,
        workspace_uid: WorkspaceUid,
        ctx: &mut ModelContext<Self>,
    ) {
        UserWorkspaces::handle(ctx).update(ctx, |user_workspaces, ctx| {
            user_workspaces.set_current_workspace_uid(workspace_uid, ctx);
        });

        // Update sqlite
        self.save_to_db([ModelEvent::SetCurrentWorkspace { workspace_uid }]);
    }
}

impl Entity for TeamUpdateManager {
    type Event = ();
}

impl SingletonEntity for TeamUpdateManager {}
