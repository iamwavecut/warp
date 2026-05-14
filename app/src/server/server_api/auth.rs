use anyhow::{Context as _, Result};
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use thiserror::Error;
use warp_core::errors::{register_error, AnyhowErrorExt, ErrorExt};
use warp_graphql::queries::get_conversation_usage::ConversationUsage;

use crate::auth::credentials::AuthToken;

use super::ServerApi;

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AuthClient: 'static + Send + Sync {
    async fn get_or_refresh_access_token(&self) -> Result<AuthToken>;

    async fn get_conversation_usage_history(
        &self,
        days: Option<i32>,
        limit: Option<i32>,
        last_updated_end_timestamp: Option<warp_graphql::scalars::Time>,
    ) -> Result<Vec<ConversationUsage>>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl AuthClient for ServerApi {
    async fn get_or_refresh_access_token(&self) -> Result<AuthToken> {
        anyhow::bail!("hosted authentication is disabled in the local-first build")
    }

    async fn get_conversation_usage_history(
        &self,
        _days: Option<i32>,
        _limit: Option<i32>,
        _last_updated_end_timestamp: Option<warp_graphql::scalars::Time>,
    ) -> Result<Vec<ConversationUsage>> {
        Ok(Vec::new())
    }
}

#[derive(Error, Debug)]
/// Error type when legacy hosted authentication cannot be accepted.
pub enum UserAuthenticationError {
    #[error("Hosted authentication denied the legacy token: {0}")]
    DeniedAccessToken(String),
    #[error("unexpected error occurred when fetching an ID token: {0:#}")]
    Unexpected(#[from] anyhow::Error),
}

impl ErrorExt for UserAuthenticationError {
    fn is_actionable(&self) -> bool {
        match self {
            UserAuthenticationError::DeniedAccessToken(err) => {
                log::info!("ignoring denied access token error: {err:#}");
                false
            }
            UserAuthenticationError::Unexpected(err) => err.is_actionable(),
        }
    }
}
register_error!(UserAuthenticationError);
