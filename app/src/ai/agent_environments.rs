use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubRepo {
    /// Repository owner.
    pub owner: String,
    /// Repository name.
    pub repo: String,
}

impl GithubRepo {
    pub fn new(owner: String, repo: String) -> Self {
        Self { owner, repo }
    }
}

impl fmt::Display for GithubRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BaseImage {
    DockerImage(String),
}

impl fmt::Display for BaseImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BaseImage::DockerImage(s) => s.fmt(f),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GcpProviderConfig {
    pub project_number: String,
    pub workload_identity_federation_pool_id: String,
    pub workload_identity_federation_provider_id: String,
    /// Service account email for impersonation. When set, the federated token
    /// is exchanged for a service account access token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_email: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AwsProviderConfig {
    pub role_arn: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct ProvidersConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcp: Option<GcpProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws: Option<AwsProviderConfig>,
}

impl ProvidersConfig {
    pub fn is_empty(&self) -> bool {
        self.gcp.is_none() && self.aws.is_none()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
/// An AmbientAgentEnvironment represents an environment that we run an agent in.
pub struct AmbientAgentEnvironment {
    /// Environment name.
    #[serde(default)]
    pub name: String,
    /// Optional description of the environment (max 240 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// List of GitHub repositories.
    #[serde(default)]
    pub github_repos: Vec<GithubRepo>,
    /// Base image specification.
    #[serde(flatten)]
    pub base_image: BaseImage,
    /// List of setup commands to run after cloning.
    #[serde(default)]
    pub setup_commands: Vec<String>,
    /// Optional provider configurations for automatic auth.
    #[serde(default, skip_serializing_if = "ProvidersConfig::is_empty")]
    pub providers: ProvidersConfig,
}
