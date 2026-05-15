use anyhow::Result;
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

use crate::auth::credentials::AuthToken;

use super::ServerApi;

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AuthClient: 'static + Send + Sync {
    async fn get_or_refresh_access_token(&self) -> Result<AuthToken>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl AuthClient for ServerApi {
    async fn get_or_refresh_access_token(&self) -> Result<AuthToken> {
        anyhow::bail!("hosted authentication is disabled in the local-first build")
    }
}
