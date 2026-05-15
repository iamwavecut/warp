pub mod ai;
pub mod auth;
pub mod object;
pub mod team;
pub mod workspace;

use crate::ai::agent::api::direct_openai::{self, CustomProviderRoute};
use crate::ai::get_relevant_files::api::{GetRelevantFiles, GetRelevantFilesResponse};
use crate::ai::predict::generate_ai_input_suggestions;
use crate::ai::predict::generate_ai_input_suggestions::GenerateAIInputSuggestionsRequest;
use crate::ai::predict::generate_am_query_suggestions;
use crate::ai::predict::generate_am_query_suggestions::GenerateAMQuerySuggestionsRequest;
use crate::ai::predict::predict_am_queries::{PredictAMQueriesRequest, PredictAMQueriesResponse};
use crate::auth::auth_state::AuthState;
use crate::settings::AISettings;
use ai::AIClient;
use auth::AuthClient;
use object::ObjectClient;
use team::TeamClient;
use warp_core::context_flag::ContextFlag;
use warp_core::errors::{register_error, AnyhowErrorExt, ErrorExt};
use warpui::ModelContext;
use workspace::WorkspaceClient;

use crate::settings_view;

use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset};
use instant::Instant;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use std::fmt;
use std::sync::Arc;
use warpui::Entity;
use warpui::SingletonEntity;

use super::experiments::ServerExperiment;
use super::experiments::ServerExperiments;

#[derive(Debug, Clone)]
pub struct ServerTime {
    time_at_fetch: DateTime<FixedOffset>,
    fetched_at: Instant,
}

impl ServerTime {
    pub fn current_time(&self) -> DateTime<FixedOffset> {
        let elapsed = chrono::Duration::from_std(self.fetched_at.elapsed())
            .expect("duration should not be bigger than limit");
        self.time_at_fetch + elapsed
    }
}

/// Wrapper for deserialization errors. This covers both:
/// * Using `serde` directly
/// * Using `reqwest` decoding utilities
#[derive(thiserror::Error, Debug)]
pub enum DeserializationError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Transport(reqwest::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum AIApiError {
    #[error("Internal error occurred at transport layer.")]
    Transport(#[source] reqwest::Error),

    #[error("Failed to deserialize API response.")]
    Deserialization(#[source] DeserializationError),

    #[error("No context found on context search.")]
    NoContextFound,

    #[error("Failed with status code {0}: {1}")]
    ErrorStatus(http::StatusCode, String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<http_client::ResponseError> for AIApiError {
    fn from(err: http_client::ResponseError) -> Self {
        Self::from_response_error(err.source, &err.headers)
    }
}

impl From<reqwest::Error> for AIApiError {
    fn from(err: reqwest::Error) -> Self {
        Self::from_transport_error(err)
    }
}

impl From<serde_json::Error> for AIApiError {
    fn from(err: serde_json::Error) -> Self {
        AIApiError::Deserialization(err.into())
    }
}

impl AIApiError {
    /// Converts a reqwest error to an AIApiError.
    fn from_response_error(err: reqwest::Error, headers: &::http::HeaderMap) -> Self {
        let _ = headers;
        Self::from_transport_error(err)
    }

    /// Converts a transport-level reqwest error (no HTTP response) to an AIApiError.
    fn from_transport_error(err: reqwest::Error) -> Self {
        // Unfortunately, `reqwest` reports some non-decoding errors as decoding errors (e.g.
        // unexpected disconnects or timeouts while deserializing a response body). Since we
        // render deserialization and transport errors differently, we try to detect those cases
        // here.
        if err.is_timeout() {
            return AIApiError::Transport(err);
        }
        if err.is_decode() {
            #[cfg(not(target_family = "wasm"))]
            {
                use std::error::Error as _;
                let mut source = err.source();
                while let Some(underlying) = source {
                    if underlying.is::<hyper::Error>() {
                        return AIApiError::Transport(err);
                    }

                    source = underlying.source();
                }
            }

            return AIApiError::Deserialization(DeserializationError::Transport(err));
        }

        AIApiError::Transport(err)
    }

    /// Returns whether or not the error can be retried.
    pub fn is_retryable(&self) -> bool {
        // Don't retry client errors, except for timeouts and quota limits.
        fn is_retryable_status(status: http::StatusCode) -> bool {
            !status.is_client_error()
                || status == http::StatusCode::REQUEST_TIMEOUT
                || status == http::StatusCode::TOO_MANY_REQUESTS
        }

        match self {
            AIApiError::ErrorStatus(status, _) => is_retryable_status(*status),
            AIApiError::Transport(e) => {
                if let Some(status) = e.status() {
                    return is_retryable_status(status);
                }
                true
            }
            // By default, retry on error.
            _ => true,
        }
    }
}

impl ErrorExt for AIApiError {
    fn is_actionable(&self) -> bool {
        match self {
            AIApiError::Deserialization(_) => true,
            AIApiError::Transport(error) => error.is_actionable(),
            AIApiError::Other(error) => error.is_actionable(),
            AIApiError::ErrorStatus(_, _) => self.is_retryable(),
            AIApiError::NoContextFound => false,
        }
    }
}
register_error!(AIApiError);

fn parse_local_ai_json<T>(content: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let trimmed = content.trim();
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }

    if let Some(fenced) = strip_json_code_fence(trimmed) {
        if let Ok(value) = serde_json::from_str(fenced) {
            return Ok(value);
        }
    }

    if let Some(slice) = json_like_slice(trimmed) {
        if let Ok(value) = serde_json::from_str(slice) {
            return Ok(value);
        }
    }

    serde_json::from_str(trimmed).map_err(Into::into)
}

fn strip_json_code_fence(content: &str) -> Option<&str> {
    let fenced = content.strip_prefix("```")?.trim_start();
    let fenced = fenced.strip_prefix("json").unwrap_or(fenced).trim_start();
    let end = fenced.rfind("```")?;
    Some(fenced[..end].trim())
}

fn json_like_slice(content: &str) -> Option<&str> {
    let object_start = content.find('{');
    let array_start = content.find('[');
    let start = match (object_start, array_start) {
        (Some(object_start), Some(array_start)) => object_start.min(array_start),
        (Some(object_start), None) => object_start,
        (None, Some(array_start)) => array_start,
        (None, None) => return None,
    };
    let end = match content.as_bytes().get(start) {
        Some(b'{') => content.rfind('}')?,
        Some(b'[') => content.rfind(']')?,
        _ => return None,
    };
    (end >= start).then_some(content[start..=end].trim())
}

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        // `Send` is an unnecessary bound when targeting wasm because the browser is single-threaded
        // and we don't leverage WebWorkers for async execution in WoW.
        pub type AIOutputStream<T> = futures::stream::LocalBoxStream<'static, Result<T, Arc<AIApiError>>>;
    } else {
        pub type AIOutputStream<T> = futures::stream::BoxStream<'static, Result<T, Arc<AIApiError>>>;
    }
}

/// An event related to the server API itself (and not a particular API call).
/// Most errors should be handled in callbacks to individual APIs, rather than sent over the
/// server API channel.
#[derive(Clone)]
pub enum ServerApiEvent {
    /// The current bearer token was refreshed.
    AccessTokenRefreshed {
        #[cfg_attr(target_family = "wasm", allow(dead_code))]
        token: String,
    },
}

impl fmt::Debug for ServerApiEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessTokenRefreshed { .. } => f
                .debug_struct("AccessTokenRefreshed")
                .field("token", &"<redacted>")
                .finish(),
        }
    }
}

/// Local placeholder for APIs that were backed by Warp-hosted services upstream.
///
/// Prefer NOT adding new methods directly on this struct; instead, add to one of the existing
/// client trait objects, or create your own. This helps keep `ServerApi` from being overloaded
/// with disparate types of calls, and allows you to mock methods in tests.
pub struct ServerApi {
    client: Arc<http_client::Client>,
    last_server_time: Arc<Mutex<Option<ServerTime>>>,
    local_ai_route: Arc<Mutex<Option<CustomProviderRoute>>>,
}

impl ServerApi {
    fn backend_disabled_error() -> anyhow::Error {
        anyhow!("Warp backend APIs are disabled in this local-first build")
    }

    fn backend_disabled_ai_error() -> AIApiError {
        AIApiError::Other(Self::backend_disabled_error())
    }

    fn new(
        _auth_state: Arc<AuthState>,
        _event_sender: async_channel::Sender<ServerApiEvent>,
        _agent_source: Option<ai::AgentSource>,
    ) -> Self {
        Self {
            client: Arc::new(http_client::Client::new()),
            last_server_time: Arc::new(Mutex::new(None)),
            local_ai_route: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    fn new_for_test() -> Self {
        Self {
            client: Arc::new(http_client::Client::new_for_test()),
            last_server_time: Arc::new(Mutex::new(None)),
            local_ai_route: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_local_ai_route(route: CustomProviderRoute) -> Self {
        Self {
            client: Arc::new(http_client::Client::new_for_test()),
            last_server_time: Arc::new(Mutex::new(None)),
            local_ai_route: Arc::new(Mutex::new(Some(route))),
        }
    }

    pub(crate) fn refresh_local_ai_route(&self, ctx: &warpui::AppContext) {
        let route = direct_openai::default_custom_provider_route(
            &AISettings::as_ref(ctx).custom_providers,
            ::ai::api_keys::ApiKeyManager::as_ref(ctx).keys(),
        );
        self.set_local_ai_route(route);
    }

    pub(crate) fn set_local_ai_route(&self, route: Option<CustomProviderRoute>) {
        *self.local_ai_route.lock() = route;
    }

    pub(crate) fn local_ai_route(&self) -> Option<CustomProviderRoute> {
        self.local_ai_route.lock().clone()
    }

    pub(crate) async fn complete_local_ai_text(
        &self,
        system_prompt: String,
        user_prompt: String,
    ) -> Result<String, AIApiError> {
        let route = self
            .local_ai_route()
            .ok_or_else(Self::backend_disabled_ai_error)?;
        direct_openai::complete_text(route, system_prompt, user_prompt).await
    }

    pub(crate) async fn complete_local_ai_json<T>(
        &self,
        system_prompt: String,
        user_prompt: String,
    ) -> Result<T, AIApiError>
    where
        T: DeserializeOwned,
    {
        let content = self
            .complete_local_ai_text(system_prompt, user_prompt)
            .await?;
        parse_local_ai_json(&content).map_err(AIApiError::Other)
    }

    pub async fn notify_login(&self) {
        log::debug!("Skipping login notification; Warp backend APIs are disabled");
    }

    pub async fn generate_ai_input_suggestions(
        &self,
        request: &GenerateAIInputSuggestionsRequest,
    ) -> Result<generate_ai_input_suggestions::GenerateAIInputSuggestionsResponseV2, AIApiError>
    {
        self.complete_local_ai_json(
            "You generate concise terminal command and agent-mode suggestions. Return only JSON matching this shape: {\"commands\":[\"command\"],\"ai_queries\":[{\"query\":\"question\",\"context_block_ids\":[]}],\"most_likely_action\":\"command|ai_query|none\"}.".to_string(),
            format!(
                "Generate suggestions for this terminal context.\n\nRequest JSON:\n{}",
                serde_json::to_string_pretty(request).map_err(AIApiError::from)?
            ),
        )
        .await
    }

    pub async fn get_relevant_files(
        &self,
        request: &GetRelevantFiles,
    ) -> Result<GetRelevantFilesResponse, AIApiError> {
        let _ = request;
        Err(Self::backend_disabled_ai_error())
    }

    pub async fn generate_am_query_suggestions(
        &self,
        request: &GenerateAMQuerySuggestionsRequest,
    ) -> Result<generate_am_query_suggestions::GenerateAMQuerySuggestionsResponse, AIApiError> {
        self.complete_local_ai_json(
            "You suggest a single useful agent-mode follow-up query for terminal output. Return only JSON matching this shape: {\"id\":\"local\",\"suggestion\":{\"simple\":{\"query\":\"question\",\"should_plan_task\":false}}}. Use null for suggestion when no useful query exists.".to_string(),
            format!(
                "Generate an agent-mode query suggestion for this terminal context.\n\nRequest JSON:\n{}",
                serde_json::to_string_pretty(request).map_err(AIApiError::from)?
            ),
        )
        .await
    }

    pub async fn predict_am_queries(
        &self,
        request: &PredictAMQueriesRequest,
    ) -> Result<PredictAMQueriesResponse, AIApiError> {
        self.complete_local_ai_json(
            "You autocomplete an agent-mode query in a terminal. Return only JSON matching this shape: {\"suggestion\":\"completed query\"}.".to_string(),
            format!(
                "Autocomplete this partial agent-mode query.\n\nRequest JSON:\n{}",
                serde_json::to_string_pretty(request).map_err(AIApiError::from)?
            ),
        )
        .await
    }

    pub async fn generate_multi_agent_output(
        &self,
        request: &warp_multi_agent_api::Request,
    ) -> std::result::Result<AIOutputStream<warp_multi_agent_api::ResponseEvent>, Arc<AIApiError>>
    {
        let _ = request;
        Err(Arc::new(Self::backend_disabled_ai_error()))
    }

    fn set_server_time(&self, server_time: ServerTime) {
        let mut last_server_time = self.last_server_time.lock();
        *last_server_time = Some(server_time);
    }

    fn cached_server_time(&self) -> Option<ServerTime> {
        let last_server_time = self.last_server_time.lock();
        last_server_time.as_ref().cloned()
    }

    /// Returns the inner `http_client::Client` used by the `ServerApi`. Callers can use this long-lived
    /// client to make requests without having to create a new client.
    pub fn http_client(&self) -> &http_client::Client {
        &self.client
    }

    pub async fn server_time(&self) -> Result<ServerTime> {
        if let Some(cached) = self.cached_server_time() {
            return Ok(cached);
        }

        let server_time = ServerTime {
            time_at_fetch: chrono::Utc::now().fixed_offset(),
            fetched_at: Instant::now(),
        };
        self.set_server_time(server_time.clone());
        Ok(server_time)
    }
}

/// A singleton entity that provides access to the global [`ServerApi`] instance,
/// or any of its implemented trait objects.
pub struct ServerApiProvider {
    server_api: Arc<ServerApi>,
}

impl ServerApiProvider {
    /// Constructs a new ServerApiProvider.
    pub fn new(
        auth_state: Arc<AuthState>,
        agent_source: Option<ai::AgentSource>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let (event_sender, event_receiver) = async_channel::bounded(10);
        let mut server_api = ServerApi::new(auth_state.clone(), event_sender, agent_source);

        if ContextFlag::NetworkLogConsole.is_enabled() {
            super::network_logging::init(
                [Arc::get_mut(&mut server_api.client)
                    .expect("guaranteed there is only one copy of client")],
                ctx,
            );
        }

        ctx.spawn_stream_local(
            event_receiver,
            move |_, event, ctx| {
                ctx.emit(event);
            },
            |_, _| {},
        );
        Self {
            server_api: Arc::new(server_api),
        }
    }

    /// Handles fetching server-side experiments by updating the appropriate app state.
    pub fn handle_experiments_fetched(
        &self,
        experiments: Vec<ServerExperiment>,
        ctx: &mut ModelContext<Self>,
    ) {
        ServerExperiments::handle(ctx).update(ctx, |state, ctx| {
            state.apply_latest_state(experiments, ctx);
        });

        settings_view::handle_experiment_change(ctx);
    }

    /// Constructs a new SeverApiProvider for tests.
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            server_api: Arc::new(ServerApi::new_for_test()),
        }
    }

    /// Returns a handle to the underlying [`ServerApi`] object.
    /// Prefer retrieving a specific trait object related to the methods you're calling.
    pub fn get(&self) -> Arc<ServerApi> {
        self.server_api.clone()
    }

    pub fn get_auth_client(&self) -> Arc<dyn AuthClient> {
        self.server_api.clone()
    }

    pub fn get_workspace_client(&self) -> Arc<dyn WorkspaceClient> {
        self.server_api.clone()
    }

    pub fn get_team_client(&self) -> Arc<dyn TeamClient> {
        self.server_api.clone()
    }

    pub fn get_ai_client(&self) -> Arc<dyn AIClient> {
        self.server_api.clone()
    }

    pub fn refresh_local_ai_route(&self, ctx: &warpui::AppContext) {
        self.server_api.refresh_local_ai_route(ctx);
    }

    pub fn get_cloud_objects_client(&self) -> Arc<dyn ObjectClient> {
        self.server_api.clone()
    }

    /// Returns the shared HTTP client used by local fallback code.
    pub fn get_http_client(&self) -> Arc<http_client::Client> {
        self.server_api.client.clone()
    }
}

impl Entity for ServerApiProvider {
    type Event = ServerApiEvent;
}

impl SingletonEntity for ServerApiProvider {}
