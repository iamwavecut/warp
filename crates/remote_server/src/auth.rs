use std::sync::Arc;
use warpui::r#async::BoxFuture;
type GetAuthTokenFn = dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync;
type RemoteServerIdentityKeyFn = dyn Fn() -> String + Send + Sync;
/// App-supplied authentication and preference context for transport-agnostic
/// remote-server code.
///
/// Bearer tokens are delivered only through protocol messages. Identity keys
/// are non-secret stable partition keys used to select the remote daemon's
/// socket/PID directory.
///
/// User identity is forwarded to the daemon via the `Initialize` handshake.
///
/// This keeps the `remote_server` crate decoupled from app-side auth/server API
/// types while still allowing initial connect and reconnect handshakes to fetch
/// the current app credential and preferences.
#[derive(Clone)]
pub struct RemoteServerAuthContext {
    get_auth_token: Arc<GetAuthTokenFn>,
    remote_server_identity_key: Arc<RemoteServerIdentityKeyFn>,
    user_id: String,
    user_email: String,
}

impl RemoteServerAuthContext {
    pub fn new(
        get_auth_token: impl Fn() -> BoxFuture<'static, Option<String>> + Send + Sync + 'static,
        remote_server_identity_key: impl Fn() -> String + Send + Sync + 'static,
        user_id: String,
        user_email: String,
    ) -> Self {
        Self {
            get_auth_token: Arc::new(get_auth_token),
            remote_server_identity_key: Arc::new(remote_server_identity_key),
            user_id,
            user_email,
        }
    }

    pub fn get_auth_token(&self) -> BoxFuture<'static, Option<String>> {
        (self.get_auth_token)()
    }

    pub fn remote_server_identity_key(&self) -> String {
        (self.remote_server_identity_key)()
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn user_email(&self) -> &str {
        &self.user_email
    }
}
