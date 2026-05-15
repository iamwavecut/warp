use std::env;
#[cfg(unix)]
use std::ffi::CStr;
use std::sync::Arc;

use parking_lot::RwLock;
use warpui::{AppContext, Entity, SingletonEntity};

use super::{
    credentials::Credentials,
    user::{PrincipalType, User},
    UserUid,
};

/// Describes what persistence action to take based on the current auth state.
pub(super) enum PersistAction {
    /// The user has been logged out and should be removed from secure storage.
    Remove,
    /// No persistence action is needed (e.g. API key or test credentials).
    DoNothing,
}

/// AuthState holds information about the currently-logged in user.
/// If you need to access AuthState, you can use the AuthStateProvider singleton model.
pub struct AuthState {
    /// The currently logged-in User. None if the user isn't logged in currently.
    user: RwLock<Option<User>>,

    /// The current authentication credentials.
    credentials: RwLock<Option<Credentials>>,
}

impl AuthState {
    fn new(ctx: &AppContext) -> Self {
        let _ = ctx;
        Self {
            user: RwLock::new(None),
            credentials: RwLock::new(None),
        }
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn new_for_test() -> Self {
        Self {
            user: RwLock::new(Some(User::test())),
            credentials: RwLock::new(Some(Credentials::Test)),
        }
    }

    #[cfg(test)]
    pub fn new_logged_out_for_test() -> Self {
        Self {
            user: RwLock::new(None),
            credentials: RwLock::new(None),
        }
    }

    /// Creates and initializes auth state for the local fork.
    ///
    /// There is no remote account branch in this build: every app session runs
    /// as the local system user with local unauthenticated credentials.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub fn initialize(ctx: &AppContext) -> Self {
        let state = Self::new(ctx);
        state.set_user(Some(User::local()));
        state.set_credentials(Some(Credentials::Local));
        state
    }

    /// Determines the appropriate persistence action based on the current auth state.
    pub(super) fn persist_action(&self) -> PersistAction {
        let user = self.user.read().clone();
        let credentials = self.credentials.read().clone();

        match (user, credentials) {
            // Remove persisted auth state if it is unset in-memory.
            (None, None) => PersistAction::Remove,
            // Do not persist if using API keys, session cookies, or test credentials.
            (Some(_), Some(Credentials::SessionCookie)) => PersistAction::DoNothing,
            #[cfg(any(test, feature = "integration_tests"))]
            (Some(_), Some(Credentials::Test)) => PersistAction::DoNothing,
            (Some(_), Some(Credentials::Local)) => PersistAction::DoNothing,
            // Credentials without a user, or user without credentials - transient states
            // during initialization or refresh; no persistence action needed.
            (None, Some(_)) | (Some(_), None) => PersistAction::DoNothing,
        }
    }

    /// Sets the user. This should only be called by the AuthManager, to ensure
    /// side-effects are handled properly (e.g. notifying other models, persisting
    /// the user to secure storage, etc.).
    pub(super) fn set_user(&self, user: Option<User>) {
        *self.user.write() = user;
    }

    /// Returns the current credentials.
    pub fn credentials(&self) -> Option<Credentials> {
        self.credentials.read().clone()
    }

    pub fn global_skills(&self) -> Vec<String> {
        self.user
            .read()
            .as_ref()
            .map(|user| user.global_skills.clone())
            .unwrap_or_default()
    }

    /// Sets the credentials. Should only be called within the auth module.
    pub(super) fn set_credentials(&self, credentials: Option<Credentials>) {
        *self.credentials.write() = credentials;
    }

    /// Determines whether the user should be considered as logged in.
    pub fn is_logged_in(&self) -> bool {
        let _ = self;
        true
    }

    /// Returns the cached access token, if any exists. This method *will not* check if the JWT is
    /// still valid! Usually, you want to use [`ServerApi::get_or_refresh_access_token`] instead!
    pub fn get_access_token_ignoring_validity(&self) -> Option<String> {
        let credentials = self.credentials.read();
        credentials.as_ref()?.bearer_token().bearer_token()
    }

    pub fn apply_remote_server_auth_context(
        &self,
        _auth_token: String,
        _user_id: String,
        _user_email: String,
    ) {
    }

    pub fn set_remote_server_bearer_token(&self, _auth_token: String) {}

    /// Returns the user's display name.
    pub fn username_for_display(&self) -> Option<String> {
        let _ = self;
        Some(local_system_display_name())
    }

    /// Returns the user's display name, does NOT fall back to email.
    pub fn display_name(&self) -> Option<String> {
        let _ = self;
        Some(local_system_display_name())
    }

    /// Returns the user's email if this local profile has one.
    pub fn user_email(&self) -> Option<String> {
        let _ = self;
        None
    }

    /// Returns whether the user considered onboarded to Warp.
    pub fn is_onboarded(&self) -> Option<bool> {
        self.user.read().as_ref().map(|user| user.is_onboarded)
    }

    /// Returns the user's email domain (anything after the @ sign of their email).
    pub fn user_email_domain(&self) -> Option<String> {
        self.user.read().as_ref().map(|user| {
            user.metadata
                .email
                .clone()
                .split('@')
                .nth(1)
                .unwrap_or("")
                .to_string()
        })
    }

    /// Returns the user's profile photo URL, if one exists.
    pub fn user_photo_url(&self) -> Option<String> {
        let _ = self;
        None
    }

    /// Set whether or not the user is onboarded.
    pub fn set_is_onboarded(&self, is_onboarded: bool) {
        if let Some(user) = self.user.write().as_mut() {
            user.is_onboarded = is_onboarded;
        }
    }

    /// If the user is logged in, returns their local user id. Otherwise, returns None.
    pub fn user_id(&self) -> Option<UserUid> {
        self.user.read().as_ref().map(|user| user.local_id)
    }

    /// Returns whether or not the user is on a work domain.
    /// This calculation is done on the server, using a list of
    pub fn is_on_work_domain(&self) -> Option<bool> {
        self.user.read().as_ref().map(|user| user.is_on_work_domain)
    }

    /// Returns the type of principal (user or service account).
    pub fn principal_type(&self) -> Option<PrincipalType> {
        self.user.read().as_ref().map(|user| user.principal_type)
    }

    /// Returns whether the authenticated principal is a service account.
    pub fn is_service_account(&self) -> bool {
        matches!(self.principal_type(), Some(PrincipalType::ServiceAccount))
    }
}

/// AuthStateProvider is a singleton model which provides a reference to the global AuthState.
pub struct AuthStateProvider {
    auth_state: Arc<AuthState>,
}

impl AuthStateProvider {
    pub fn new(auth_state: Arc<AuthState>) -> Self {
        Self { auth_state }
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            auth_state: Arc::new(AuthState::new_for_test()),
        }
    }

    /// Constructs a provider backed by a fully logged-out `AuthState` (no user,
    /// no credentials). Used by unit tests that need to exercise code paths
    /// gated on `AuthState::user_id()` / `UserWorkspaces::personal_drive()`
    /// returning `None`.
    #[cfg(test)]
    pub fn new_logged_out_for_test() -> Self {
        Self {
            auth_state: Arc::new(AuthState {
                user: RwLock::new(None),
                credentials: RwLock::new(None),
            }),
        }
    }

    pub fn get(&self) -> &Arc<AuthState> {
        &self.auth_state
    }
}

impl Entity for AuthStateProvider {
    type Event = ();
}

impl SingletonEntity for AuthStateProvider {}

fn local_system_display_name() -> String {
    let (full_name, username) = local_system_user_names();
    pick_local_system_display_name(full_name, username.or_else(env_username))
}

fn pick_local_system_display_name(full_name: Option<String>, username: Option<String>) -> String {
    full_name
        .and_then(trimmed_non_empty)
        .or_else(|| username.and_then(trimmed_non_empty))
        .unwrap_or_else(|| "User".to_owned())
}

fn env_username() -> Option<String> {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|key| env::var(key).ok().and_then(trimmed_non_empty))
}

fn trimmed_non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(unix)]
fn local_system_user_names() -> (Option<String>, Option<String>) {
    // SAFETY: getpwuid returns a pointer owned by libc. We copy C strings
    // immediately and tolerate null pointers or non-UTF8 data by falling back.
    unsafe {
        let passwd = libc::getpwuid(libc::getuid());
        if passwd.is_null() {
            return (None, None);
        }

        let username = c_string((*passwd).pw_name).and_then(trimmed_non_empty);
        let full_name = c_string((*passwd).pw_gecos)
            .and_then(|gecos| gecos.split(',').next().map(str::to_owned))
            .and_then(|name| expand_gecos_name(name, username.as_deref()))
            .and_then(trimmed_non_empty);

        (full_name, username)
    }
}

#[cfg(not(unix))]
fn local_system_user_names() -> (Option<String>, Option<String>) {
    (None, None)
}

#[cfg(unix)]
fn c_string(ptr: *const libc::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }

    // SAFETY: caller gives us a pointer from libc's passwd entry and we check
    // for null before copying it into an owned Rust string.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

#[cfg(unix)]
fn expand_gecos_name(name: String, username: Option<&str>) -> Option<String> {
    if !name.contains('&') {
        return Some(name);
    }

    let username = username?;
    let mut chars = username.chars();
    let expanded_username = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect::<String>())
        .unwrap_or_else(|| username.to_owned());
    Some(name.replace('&', &expanded_username))
}

#[cfg(test)]
mod tests {
    use super::pick_local_system_display_name;

    #[test]
    fn local_display_name_prefers_system_full_name() {
        assert_eq!(
            pick_local_system_display_name(
                Some("  Ada Lovelace  ".to_owned()),
                Some("ada".to_owned())
            ),
            "Ada Lovelace"
        );
    }

    #[test]
    fn local_display_name_falls_back_to_username() {
        assert_eq!(
            pick_local_system_display_name(Some(" ".to_owned()), Some("wavecut".to_owned())),
            "wavecut"
        );
    }
}
