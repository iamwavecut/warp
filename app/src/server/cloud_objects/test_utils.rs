use std::sync::{
    mpsc::{sync_channel, Receiver},
    Arc,
};

use settings::manager::SettingsManager;
use warp_core::execution_mode::{AppExecutionMode, ExecutionMode};
use warpui::{App, ModelHandle, SingletonEntity};

use crate::{
    auth::{auth_manager::AuthManager, AuthStateProvider},
    cloud_object::model::{
        actions::ObjectActions,
        persistence::{CloudModel, CloudModelEvent},
    },
    network::NetworkStatus,
    persistence::ModelEvent,
    server::{
        server_api::{
            object::{MockObjectClient, ObjectClient},
            ServerApiProvider,
        },
        sync_queue::SyncQueue,
    },
    settings::PrivacySettings,
    workspaces::{
        team_tester::TeamTesterStatus, update_manager::TeamUpdateManager,
        user_profiles::UserProfiles, user_workspaces::UserWorkspaces,
    },
};

use super::update_manager::UpdateManager;

/// The size of the bounded channel that we use to queue persistence/sqlite-related events.
const CHANNEL_SIZE: usize = 128;

pub struct UpdateManagerStruct {
    pub update_manager: ModelHandle<UpdateManager>,
    pub receiver: Receiver<ModelEvent>,
    pub cloud_model_events: async_channel::Receiver<CloudModelEvent>,
}

pub fn initialize_app(app: &mut App) {
    app.add_singleton_model(|ctx| AppExecutionMode::new(ExecutionMode::App, false, ctx));

    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| SettingsManager::default());
    app.add_singleton_model(TeamTesterStatus::mock);
    app.update(crate::settings::init_and_register_user_preferences);
    // This ServerApiProvider is used for the PrivacySettings model, but not the UpdateManager
    // under test.
    app.add_singleton_model(|_| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
    app.update(PrivacySettings::register_singleton);
    app.add_singleton_model(CloudModel::mock);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(TeamUpdateManager::mock);
    app.add_singleton_model(|_| ObjectActions::new(Vec::new()));
}

pub fn create_update_manager_struct(
    app: &mut App,
    server_api: Arc<dyn ObjectClient>,
) -> UpdateManagerStruct {
    let (sender, receiver) = sync_channel(CHANNEL_SIZE);

    // the sync queue can't be mocked; needs to use the same server_api as the update_manager
    app.add_singleton_model(|ctx| SyncQueue::new(Default::default(), server_api.clone(), ctx));
    let update_manager =
        app.add_singleton_model(|ctx| UpdateManager::new(Some(sender.clone()), server_api, ctx));

    app.add_singleton_model(|_| UserProfiles::new(Vec::new()));

    // set up the sync queue in a dequeueing state
    SyncQueue::handle(app).update(app, |sync_queue, ctx| {
        sync_queue.start_dequeueing(ctx);
    });

    let cloud_model_events = app.update(|ctx| {
        let (tx, rx) = async_channel::unbounded();
        ctx.subscribe_to_model(&CloudModel::handle(ctx), move |_, event, _| {
            let _ = tx.try_send(event.clone());
        });
        rx
    });

    UpdateManagerStruct {
        update_manager,
        receiver,
        cloud_model_events,
    }
}

/// Creates a baseline [`MockObjectClient`] with common mocks.
pub fn mock_server_api() -> MockObjectClient {
    MockObjectClient::new()
}
