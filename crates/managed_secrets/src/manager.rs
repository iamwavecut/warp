use std::{collections::HashMap, future::Future, sync::Arc};

use warp_graphql::managed_secrets::ManagedSecret;
use warpui::{Entity, SingletonEntity};

use crate::{
    ManagedSecretValue,
    client::{IdentityTokenOptions, ManagedSecretsClient, SecretOwner, TaskIdentityToken},
};

/// Singleton model for working with Warp-managed secrets.
pub struct ManagedSecretManager {
    client: Arc<dyn ManagedSecretsClient>,
    actor_provider: Arc<dyn ActorProvider>,
}

pub trait ActorProvider: Send + Sync + 'static {
    fn actor_uid(&self) -> Option<String>;
}

impl ManagedSecretManager {
    pub fn new(
        client: Arc<dyn ManagedSecretsClient>,
        actor_provider: Arc<dyn ActorProvider>,
    ) -> Self {
        Self {
            client,
            actor_provider,
        }
    }

    pub fn create_secret(
        &self,
        owner: SecretOwner,
        name: String,
        value: ManagedSecretValue,
        description: Option<String>,
    ) -> impl Future<Output = anyhow::Result<ManagedSecret>> + use<> {
        let client = self.client.clone();
        let actor_provider = self.actor_provider.clone();
        async move {
            let _ = (client, actor_provider, owner, name, value, description);
            Err(local_managed_secrets_disabled())
        }
    }

    pub fn delete_secret(
        &self,
        owner: SecretOwner,
        name: String,
    ) -> impl Future<Output = anyhow::Result<()>> + use<> {
        let client = self.client.clone();
        async move {
            let _ = (client, owner, name);
            Err(local_managed_secrets_disabled())
        }
    }

    pub fn update_secret(
        &self,
        owner: SecretOwner,
        name: String,
        value: Option<ManagedSecretValue>,
        description: Option<String>,
    ) -> impl Future<Output = anyhow::Result<ManagedSecret>> + use<> {
        let client = self.client.clone();
        let actor_provider = self.actor_provider.clone();
        async move {
            let _ = (client, actor_provider, owner, name, value, description);
            Err(local_managed_secrets_disabled())
        }
    }

    pub fn list_secrets(&self) -> impl Future<Output = anyhow::Result<Vec<ManagedSecret>>> + use<> {
        let client = self.client.clone();
        async move {
            let _ = client;
            Ok(vec![])
        }
    }

    pub fn get_task_secrets(
        &self,
        task_id: String,
    ) -> impl Future<Output = anyhow::Result<HashMap<String, ManagedSecretValue>>> + use<> {
        let client = self.client.clone();
        async move {
            let _ = (client, task_id);
            Ok(HashMap::new())
        }
    }

    pub fn issue_task_identity_token(
        &self,
        options: IdentityTokenOptions,
    ) -> impl Future<Output = anyhow::Result<TaskIdentityToken>> + use<> {
        let client = self.client.clone();
        async move {
            let _ = (client, options);
            Err(local_managed_secrets_disabled())
        }
    }
}

fn local_managed_secrets_disabled() -> anyhow::Error {
    anyhow::anyhow!("Warp-managed secrets are disabled in the local-first build")
}

impl Entity for ManagedSecretManager {
    type Event = ();
}

impl SingletonEntity for ManagedSecretManager {}
