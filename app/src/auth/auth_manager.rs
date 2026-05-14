pub(super) mod user_persistence;

use std::sync::Arc;

use warpui::{Entity, ModelContext, SingletonEntity};

use super::auth_state::{AuthState, PersistAction};
use super::auth_view_modal::{AuthRedirectPayload, AuthViewVariant};
use super::AuthStateProvider;
use crate::interaction_sources::AnonymousUserSignupEntrypoint;
use crate::server::server_api::{
    auth::{AuthClient, UserAuthenticationError},
    ServerApi,
};
#[cfg(target_family = "wasm")]
use crate::uri::browser_url_handler::{parse_current_url, update_browser_url};
#[cfg(target_family = "wasm")]
use url::Url;
use user_persistence::PersistedUser;

#[derive(Debug)]
pub enum AuthManagerEvent {
    /// Successfully authenticated a user with no errors.
    AuthComplete,
    /// Failed to authenticate a user, due to a particular `UserAuthenticationError`.
    AuthFailed(UserAuthenticationError),
    /// The user chose to skip login entirely.
    SkippedLogin,
    /// The user is anonymous and has attempted to access a login-gated feature or link.
    AttemptedLoginGatedFeature {
        auth_view_variant: AuthViewVariant,
    },
    // The current user is anonymous and the client has received a browser intent to sign in with a different Warp account.
    // Holds an auth payload from the received browser intent.
    LoginOverrideDetected(AuthRedirectPayload),
}

pub type LoginGatedFeature = &'static str;

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

    /// Fetches and ultimately sets the user's auth state from an auth payload.
    /// Typically, this function is triggered when a user clicks the intent link from their browser
    /// back to Warp after login (or pastes the URL in the app).
    pub fn initialize_user_from_auth_payload(
        &mut self,
        auth_payload: AuthRedirectPayload,
        enforce_state_validation: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let _ = (auth_payload, enforce_state_validation, ctx);
        log::info!("Ignoring remote auth payload in local workflow");
    }

    pub fn resume_interrupted_auth_payload(
        &mut self,
        auth_payload: AuthRedirectPayload,
        ctx: &mut ModelContext<Self>,
    ) {
        let _ = (auth_payload, ctx);
        log::info!("Ignoring interrupted remote auth payload in local workflow");
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

    /// Helper function for logging out the user.
    /// NOTE: You probably want to call auth::log_out instead; this only manages the auth state,
    /// it doesn't shut down any other user-dependent parts of the app.
    /// TODO(jeff): Can we move those pieces in here?
    pub(super) fn log_out(&mut self, ctx: &mut ModelContext<Self>) {
        let _ = ctx;
        log::info!("Ignoring logout in local workflow");
    }

    pub fn attempt_login_gated_feature(
        &self,
        feature: LoginGatedFeature,
        auth_view_variant: AuthViewVariant,
        ctx: &mut ModelContext<Self>,
    ) {
        let _ = (feature, auth_view_variant, ctx);
    }

    pub fn anonymous_user_hit_drive_object_limit(&self, ctx: &mut ModelContext<Self>) {
        let _ = ctx;
    }

    pub fn initiate_anonymous_user_linking(
        &self,
        entrypoint: AnonymousUserSignupEntrypoint,
        ctx: &mut ModelContext<Self>,
    ) {
        let _ = (entrypoint, ctx);
    }

    pub fn copy_anonymous_user_linking_url_to_clipboard(&self, ctx: &mut ModelContext<Self>) {
        let _ = ctx;
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
    type Event = AuthManagerEvent;
}

impl SingletonEntity for AuthManager {}

#[cfg(test)]
#[path = "auth_manager_test.rs"]
mod auth_manager_test;
