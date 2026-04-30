use std::{result::Result as StdResult, sync::Arc};

use anyhow::{anyhow, Context as _, Result};
use async_trait::async_trait;
use cynic::{MutationBuilder, QueryBuilder};
use firebase::{FetchAccessTokenResponse, FirebaseError};
use futures::FutureExt;
use instant::Duration;
#[cfg(test)]
use mockall::{automock, predicate::*};
use oauth2::TokenResponse;
use thiserror::Error;
use warp_core::errors::{AnyhowErrorExt, ErrorExt};
use warp_graphql::client::Operation;
use warp_graphql::mutations::expire_api_key::{
    ExpireApiKey, ExpireApiKeyResult, ExpireApiKeyVariables,
};
use warp_graphql::queries::get_conversation_usage::{
    ConversationUsage, GetConversationUsage, GetConversationUsageVariables, UserResult,
};

use warp_graphql::mutations::set_user_is_onboarded::{
    SetUserIsOnboarded, SetUserIsOnboardedResult, SetUserIsOnboardedVariables,
};
use warp_graphql::mutations::update_user_settings::{
    UpdateUserSettings, UpdateUserSettingsInput, UpdateUserSettingsResult,
    UpdateUserSettingsVariables,
};
use warp_graphql::mutations::{
    create_anonymous_user::{
        AnonymousUserType, CreateAnonymousUser, CreateAnonymousUserResult,
        CreateAnonymousUserVariables,
    },
    generate_api_key::{
        GenerateApiKey, GenerateApiKeyInput, GenerateApiKeyResult, GenerateApiKeyVariables,
    },
    mint_custom_token::{MintCustomTokenResult, MintCustomTokenVariables},
};
use warp_graphql::object_permissions::OwnerType;
use warp_graphql::queries::api_keys::{
    ApiKeyProperties, ApiKeyPropertiesResult, ApiKeys, ApiKeysVariables,
};
use warp_graphql::queries::get_user::{GetUser, GetUserVariables, UserOutput as GqlUserOutput};
use warp_graphql::queries::get_user_settings::{GetUserSettings, GetUserSettingsVariables};
use warpui::r#async::BoxFuture;

use crate::auth::UserUid;
use crate::server::graphql::{default_request_options, get_user_facing_error_message};
use crate::server::ids::ApiKeyUid;
use crate::server::server_api::register_error;
use crate::server::server_api::EXPERIMENT_ID_HEADER;
use crate::settings::PrivacySettingsSnapshot;
use crate::{
    auth::{
        credentials::{AuthToken, Credentials, FirebaseToken, LoginToken, RefreshToken},
        user::FirebaseAuthTokens,
        user::User,
    },
    channel::ChannelState,
    convert_to_server_experiment,
    server::{
        datetime_ext::DateTimeExt as _, experiments::ServerExperiment,
        graphql::get_request_context, server_api::ServerApiEvent,
    },
};

use super::ServerApi;

/// Error messages returned from the Firebase REST API when attempting to convert a refresh token
/// into an access token that indicate the user's token is in an errored state.
/// These are "soft" errors because the user likely just needs to log in again.
/// See https://firebase.google.com/docs/reference/rest/auth#section-refresh-token.
static FETCH_ACCESS_TOKEN_SOFT_ERROR_MESSAGES: &[&str] = &[
    "TOKEN_EXPIRED",
    "INVALID_REFRESH_TOKEN",
    "MISSING_REFRESH_TOKEN",
];

/// Error messages returned from the Firebase REST API when attempting to convert a refresh token
/// into an access token that indicate the user's account is in an errored state.
/// These are "hard" errors because the user likely can no longer sign in with their account,
/// for example if it were disabled or deleted.
/// See https://firebase.google.com/docs/reference/rest/auth#section-refresh-token.
static FETCH_ACCESS_TOKEN_HARD_ERROR_MESSAGES: &[&str] = &["USER_DISABLED", "USER_NOT_FOUND"];

const FETCH_ACCESS_TOKEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Header key for the ambient workload token attached to multi-agent requests.
pub const AMBIENT_WORKLOAD_TOKEN_HEADER: &str = "X-Warp-Ambient-Workload-Token";

/// Header key for the cloud agent task ID attached to requests from ambient agents.
pub const CLOUD_AGENT_ID_HEADER: &str = "X-Warp-Cloud-Agent-ID";

/// Duration for which the ambient workload token is valid (3 hours).
const AMBIENT_WORKLOAD_TOKEN_DURATION: Duration = Duration::from_secs(3 * 60 * 60);

/// User settings that are currently 'synced' (e.g. stored server-side) on a per-user basis.
#[derive(Copy, Clone, Debug, Default)]
pub struct SyncedUserSettings {
    pub is_cloud_conversation_storage_enabled: bool,
}

/// Results of an attempt to fetch the current user.
pub struct FetchUserResult {
    pub user: User,
    /// The credentials used to authenticate this user.
    pub credentials: Credentials,
    pub server_experiments: Vec<ServerExperiment>,
    /// Whether this attempt to fetch the user was for refreshing an existing logged-in user.
    pub from_refresh: bool,
    /// LLM model choices for this user.
    pub llms: crate::ai::llms::ModelsByFeature,
}

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AuthClient: 'static + Send + Sync {
    /// Creates an anonymous user, who is allowed to use Warp but may lack the ability
    /// to interact with particular features.
    async fn create_anonymous_user(
        &self,
        referral_code: Option<String>,
        anonymous_user_type: AnonymousUserType,
    ) -> Result<CreateAnonymousUserResult>;

    /// Returns the cached access token, if it is still valid. If it has expired, fetches a new
    /// access token using the user's refresh token, caches it, and the returns it.
    /// Returns an auth mode that may not require an Authorization header (e.g. session cookies or
    /// test credentials).
    async fn get_or_refresh_access_token(&self) -> Result<AuthToken>;

    /// Fetches data required to construct the [`User`] object. This includes the user's metadata
    /// and authentication tokens.
    async fn fetch_user(
        &self,
        token: LoginToken,
        for_refresh: bool,
    ) -> StdResult<FetchUserResult, UserAuthenticationError>;

    /// Creates and fetches an new custom token for the current user from Firebase.
    /// This only works for anonymous users, and will surface an error if the user is not anonymous.
    async fn fetch_new_custom_token(&self) -> Result<MintCustomTokenResult>;

    /// Handles the response from [`Self::fetch_new_custom_token`], returning the newly-minted custom token.
    fn on_custom_token_fetched(
        &self,
        response: Result<MintCustomTokenResult>,
    ) -> Result<String, MintCustomTokenError>;

    /// Queries warp-server for a set of the currently logged-in user's fields.
    async fn fetch_user_properties<'a>(&self, auth_token: Option<&'a str>)
        -> Result<GqlUserOutput>;

    /// Upon success, returns an `Option` containing the user's settings retrieved from the server,
    /// if any. If the fetched settings object exists but is missing required fields, or if the
    /// request itself failed, returns an error.
    async fn get_user_settings(&self) -> Result<Option<SyncedUserSettings>>;

    /// Returns conversation usage history for the current user over the past n days.
    /// If last_updated_end_timestamp is provided, only conversations with
    /// lastUpdated earlier than this timestamp are returned.
    async fn get_conversation_usage_history(
        &self,
        days: Option<i32>,
        limit: Option<i32>,
        last_updated_end_timestamp: Option<warp_graphql::scalars::Time>,
    ) -> Result<Vec<ConversationUsage>>;

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyProperties>>;

    async fn create_api_key(
        &self,
        name: String,
        team_id: Option<cynic::Id>,
        expires_at: Option<warp_graphql::scalars::Time>,
    ) -> Result<GenerateApiKeyResult>;

    async fn expire_api_key(&self, key_uid: &ApiKeyUid) -> Result<ExpireApiKeyResult>;

    async fn get_or_create_ambient_workload_token(&self) -> Result<Option<String>>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl AuthClient for ServerApi {
    async fn create_anonymous_user(
        &self,
        _referral_code: Option<String>,
        _anonymous_user_type: AnonymousUserType,
    ) -> Result<CreateAnonymousUserResult> {
        anyhow::bail!("anonymous user creation is disabled in the local-first build")
    }

    async fn get_or_refresh_access_token(&self) -> Result<AuthToken> {
        anyhow::bail!("hosted authentication is disabled in the local-first build")
    }

    async fn fetch_user(
        &self,
        _token: LoginToken,
        _for_refresh: bool,
    ) -> StdResult<FetchUserResult, UserAuthenticationError> {
        Err(UserAuthenticationError::Unexpected(anyhow!(
            "hosted authentication is disabled in the local-first build"
        )))
    }

    async fn fetch_new_custom_token(&self) -> Result<MintCustomTokenResult> {
        anyhow::bail!("custom token minting is disabled in the local-first build")
    }

    fn on_custom_token_fetched(
        &self,
        _response: Result<MintCustomTokenResult>,
    ) -> Result<String, MintCustomTokenError> {
        Err(MintCustomTokenError::Unknown)
    }

    async fn fetch_user_properties<'a>(
        &self,
        _auth_token: Option<&'a str>,
    ) -> Result<GqlUserOutput> {
        anyhow::bail!("hosted user lookup is disabled in the local-first build")
    }

    async fn get_user_settings(&self) -> Result<Option<SyncedUserSettings>> {
        Ok(None)
    }

    async fn get_conversation_usage_history(
        &self,
        _days: Option<i32>,
        _limit: Option<i32>,
        _last_updated_end_timestamp: Option<warp_graphql::scalars::Time>,
    ) -> Result<Vec<ConversationUsage>> {
        Ok(Vec::new())
    }

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyProperties>> {
        Ok(Vec::new())
    }

    async fn create_api_key(
        &self,
        _name: String,
        _team_id: Option<cynic::Id>,
        _expires_at: Option<warp_graphql::scalars::Time>,
    ) -> Result<GenerateApiKeyResult> {
        anyhow::bail!("server API key creation is disabled in the local-first build")
    }

    async fn expire_api_key(&self, _key_uid: &ApiKeyUid) -> Result<ExpireApiKeyResult> {
        anyhow::bail!("server API key expiration is disabled in the local-first build")
    }

    async fn get_or_create_ambient_workload_token(&self) -> Result<Option<String>> {
        Ok(None)
    }
}

/// Exchange a long-lived token for fresh [`Credentials`].
async fn exchange_credentials(
    client: Arc<http_client::Client>,
    token: LoginToken,
) -> StdResult<Credentials, UserAuthenticationError> {
    match token {
        LoginToken::Firebase(firebase_token) => {
            let tokens = fetch_auth_tokens(client, firebase_token).await?;
            Ok(Credentials::Firebase(tokens))
        }
        LoginToken::ApiKey(key) => Ok(Credentials::ApiKey {
            key,
            owner_type: None,
        }),
        LoginToken::SessionCookie => Ok(Credentials::SessionCookie),
    }
}

fn fetch_auth_tokens(
    client: Arc<http_client::Client>,
    token: FirebaseToken,
) -> BoxFuture<'static, StdResult<FirebaseAuthTokens, UserAuthenticationError>> {
    Box::pin(async move {
        let firebase_api_key = ChannelState::firebase_api_key();
        let url = token.access_token_url(&firebase_api_key);
        let request_body = token.access_token_request_body();
        let proxy_url = token.proxy_url(&ChannelState::server_root_url(), &firebase_api_key);
        let response = match client
            .post(&url)
            .form(&request_body)
            .timeout(FETCH_ACCESS_TOKEN_TIMEOUT)
            .send()
            .await
        {
            Ok(response) => match response.error_for_status_ref() {
                Ok(_) => Ok(response),
                Err(error) => {
                    log::warn!(
                        "Request to firebase to fetch access token completed, but was unsuccessful: {error:?}"
                    );

                    fetch_access_token_via_proxy(client, &request_body, proxy_url).await
                }
            },
            Err(error) => {
                log::warn!("Failed to make response to firebase to fetch access token: {error:?}");

                fetch_access_token_via_proxy(client, &request_body, proxy_url).await
            }
        }?;

        let response = response
            .json::<FetchAccessTokenResponse>()
            .await
            .map_err(anyhow::Error::from)?;
        match response {
            FetchAccessTokenResponse::Success {
                id_token,
                expires_in,
                refresh_token,
            } => Ok(FirebaseAuthTokens::from_response(
                id_token,
                refresh_token,
                expires_in,
            )?),
            FetchAccessTokenResponse::Error { error } => Err(error.into()),
        }
    })
}

fn fetch_access_token_via_proxy<'a>(
    client: Arc<http_client::Client>,
    request_body: &'a [(&'a str, &'a str)],
    proxy_url: String,
) -> BoxFuture<'a, Result<http_client::Response>> {
    Box::pin(async move {
        client
            .post(&proxy_url)
            .form(request_body)
            .send()
            .await
            .map_err(anyhow::Error::from)
    })
}

/// The [`oauth2::Client`] type, specialized to the endpoints that we require.
pub type OAuth2Client = oauth2::basic::BasicClient<
    oauth2::EndpointNotSet, // HasAuthUrl
    oauth2::EndpointSet,    // HasDeviceAuthUrl
    oauth2::EndpointNotSet, // HasIntrospectionUrl
    oauth2::EndpointNotSet, // HasRevocationUrl
    oauth2::EndpointSet,    // HasTokenUrl
>;

/// Intermediate type produced by converting a [`GqlUserOutput`] from the server.
struct UserProperties {
    user: User,
    server_experiments: Vec<ServerExperiment>,
    llms: crate::ai::llms::ModelsByFeature,
    api_key_owner_type: Option<OwnerType>,
}

impl From<GqlUserOutput> for UserProperties {
    fn from(user_output: GqlUserOutput) -> Self {
        let principal_type = user_output
            .principal_type
            .map(|pt| pt.into())
            .unwrap_or_default();
        let user_properties = user_output.user;

        let is_on_work_domain = user_properties.is_on_work_domain;
        let is_onboarded = user_properties.is_onboarded;
        let api_key_owner_type = user_output.api_key_owner_type;

        let linked_at = user_properties
            .anonymous_user_info
            .as_ref()
            .and_then(|info| info.linked_at);

        let anonymous_user_type = user_properties
            .anonymous_user_info
            .as_ref()
            .map(|info| info.anonymous_user_type.clone());
        let personal_object_limits = user_properties
            .anonymous_user_info
            .and_then(|info| info.personal_object_limits.clone());
        let user_profile = user_properties.profile;
        let local_id = UserUid::new(user_profile.uid.as_str());
        let needs_sso_link = user_profile.needs_sso_link;

        let server_experiments: Vec<ServerExperiment> = user_properties
            .experiments
            .and_then(|experiments| convert_to_server_experiment!(experiments))
            .unwrap_or_default();

        // Convert LLM model choices from GraphQL response
        let llms = user_properties.llms.try_into().unwrap_or_default();

        let user = User {
            is_onboarded,
            local_id,
            metadata: user_profile.into(),
            needs_sso_link,
            anonymous_user_type: anonymous_user_type.and_then(|t| t.try_into().ok()),
            is_on_work_domain,
            linked_at,
            personal_object_limits: personal_object_limits.and_then(|t| t.try_into().ok()),
            principal_type,
        };

        UserProperties {
            user,
            server_experiments,
            llms,
            api_key_owner_type,
        }
    }
}

#[derive(Error, Debug)]
/// Error type when retrieving a user and validating it against Firebase.
pub enum UserAuthenticationError {
    /// The user's refresh token is invalid. This could occur if the user authed through
    /// e.g. Google/GitHub and changed their password.
    #[error("Firebase returned a token error when fetching an ID token")]
    DeniedAccessToken(FirebaseError),
    /// The user's account is invalid. This could occur if the user requested their account
    /// be deleted per their GDPR/CCPA rights.
    #[error("Firebase returned a user error when fetching an ID token")]
    UserAccountDisabled(FirebaseError),
    #[error("Invalid state parameter in auth redirect")]
    InvalidStateParameter,
    #[error("Missing state parameter in auth redirect")]
    MissingStateParameter,
    #[error("unexpected error occurred when fetching an ID token: {0:#}")]
    Unexpected(#[from] anyhow::Error),
}

impl ErrorExt for UserAuthenticationError {
    fn is_actionable(&self) -> bool {
        match self {
            UserAuthenticationError::DeniedAccessToken(err) => {
                // If a request to our server failed because the user's refresh token
                // has expired, they should re-auth, but there's no value in reporting
                // this back to us.
                log::info!("ignoring denied access token error: {err:#}");
                false
            }
            UserAuthenticationError::UserAccountDisabled(err) => {
                // Similarly, if their account is disabled, they can't make requests.
                log::info!("ignoring user account disabled error: {err:#}");
                false
            }
            UserAuthenticationError::Unexpected(err) => err.is_actionable(),
            UserAuthenticationError::InvalidStateParameter
            | UserAuthenticationError::MissingStateParameter => {
                // For now, we're marking these as actionable, since a surplus of these errors
                // could mean that something is wrong in our login flow (e.g. we're not properly
                // passing the `state` variable back to the desktop client).
                // But in general, someone attempting to trick another into logging into their
                // account with a spoofed `state` variable is not actionable.
                true
            }
        }
    }
}
register_error!(UserAuthenticationError);

impl From<FirebaseError> for UserAuthenticationError {
    fn from(error: FirebaseError) -> Self {
        if FETCH_ACCESS_TOKEN_SOFT_ERROR_MESSAGES.contains(&error.message.as_str()) {
            UserAuthenticationError::DeniedAccessToken(error)
        } else if FETCH_ACCESS_TOKEN_HARD_ERROR_MESSAGES.contains(&error.message.as_str()) {
            UserAuthenticationError::UserAccountDisabled(error)
        } else {
            UserAuthenticationError::Unexpected(
                anyhow::Error::from(error)
                    .context("Failed to exchange refresh token with access token."),
            )
        }
    }
}

#[derive(Error, Debug)]
/// Error type when creating anonymous users
pub enum AnonymousUserCreationError {
    #[error("The network request to create the anonymous user failed")]
    CreationFailed,

    #[error("Received a user facing error: {0}")]
    UserFacingError(String),

    /// Failure that occurs after the user is created, but the ID token could not be fetched.
    #[error("The user was created, but the ID token could not be fetched")]
    UserAuthenticationFailed(#[from] UserAuthenticationError),

    #[error("Failed to create anonymous user with unknown error")]
    Unknown,
}

#[derive(Error, Debug)]
/// Error type when minting a new custom token for an anonymous user
pub enum MintCustomTokenError {
    #[error("Received a user facing error: {0}")]
    UserFacingError(String),
    #[error("Failed to create new custom token with unknown error")]
    Unknown,
}

#[cfg(test)]
#[path = "auth_test.rs"]
mod tests;
