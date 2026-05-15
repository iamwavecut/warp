pub(super) mod user_persistence;

use std::sync::Arc;

use warpui::{Entity, ModelContext, SingletonEntity};

use super::auth_state::{AuthState, PersistAction};
use super::AuthStateProvider;
use crate::server::server_api::{auth::AuthClient, ServerApi};
use user_persistence::PersistedUser;

/// AuthManager is a singleton model which manages the currently logged-in user's state.
/// If you need to access the state, use `AuthStateProvider`.
pub struct AuthManager {
    auth_state: Arc<AuthState>,
}

impl AuthManager {
    /// Creates a new instance of the AuthManager. The auth state must already be initialized through
    /// [`AuthStateProvider`].
    pub fn new(
        server_api: Arc<ServerApi>,
        auth_client: Arc<dyn AuthClient>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
        let _ = (server_api, auth_client);

        Self { auth_state }
    }

    #[cfg(test)]
    pub fn new_for_test(ctx: &mut ModelContext<Self>) -> Self {
        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();

        Self { auth_state }
    }

    #[cfg(target_family = "wasm")]
    pub fn initialize_user_from_session_cookie(&self, ctx: &mut ModelContext<Self>) {
        let _ = ctx;
    }

    /// Refreshes the user's auth state using their existing credentials.
    pub fn refresh_user(&self, ctx: &mut ModelContext<Self>) {
        let _ = ctx;
        log::info!("Skipping remote user refresh in local workflow");
    }

    /// Persists (or removes) the current user and credentials to/from secure storage,
    /// based on the current auth state.
    fn persist(&self, ctx: &mut ModelContext<Self>) {
        match self.auth_state.persist_action() {
            PersistAction::Remove => {
                let _ = PersistedUser::remove_from_secure_storage(ctx).map_err(|err| {
                    log::warn!("Unable to clear user from secure storage: {err:?}");
                });
            }
            PersistAction::DoNothing => {}
        }
    }

    /// Sets the local user as onboarded and persists the user data.
    pub fn set_user_onboarded(&self, ctx: &mut ModelContext<Self>) {
        self.auth_state.set_is_onboarded(true);
        self.persist(ctx);
    }
}

#[derive(Clone, Debug)]
pub struct PersistedCurrentUserInformation {
    pub email: String,
}

impl Entity for AuthManager {
    type Event = ();
}

impl SingletonEntity for AuthManager {}

#[cfg(test)]
#[path = "auth_manager_test.rs"]
mod auth_manager_test;
