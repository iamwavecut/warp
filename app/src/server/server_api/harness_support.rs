#![cfg_attr(target_family = "wasm", expect(dead_code))]

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

use super::ServerApi;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::artifacts::Artifact;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct UploadTarget {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SnapshotUploadRequest {
    pub files: Vec<SnapshotFileInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SnapshotFileInfo {
    pub filename: String,
    pub mime_type: String,
}

#[derive(serde::Serialize)]
pub struct ResolvePromptAttachedSkill {
    pub name: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ResolvePromptRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<ResolvePromptAttachedSkill>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments_dir: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct ResolvedHarnessPrompt {
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub resumption_prompt: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct ReportArtifactResponse {
    pub artifact_uid: String,
}

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait HarnessSupportClient: 'static + Send + Sync {
    async fn create_external_conversation(&self, format: &str) -> Result<AIConversationId>;
    async fn get_transcript_upload_target(
        &self,
        conversation_id: &AIConversationId,
    ) -> Result<UploadTarget>;
    async fn get_block_snapshot_upload_target(
        &self,
        conversation_id: &AIConversationId,
    ) -> Result<UploadTarget>;
    async fn resolve_prompt(&self, request: ResolvePromptRequest) -> Result<ResolvedHarnessPrompt>;
    async fn report_artifact(&self, artifact: &Artifact) -> Result<ReportArtifactResponse>;
    async fn notify_user(&self, message: &str) -> Result<()>;
    async fn finish_task(&self, success: bool, summary: &str) -> Result<()>;
    async fn get_snapshot_upload_targets(
        &self,
        request: &SnapshotUploadRequest,
    ) -> Result<Vec<UploadTarget>>;
    async fn fetch_transcript(&self) -> Result<bytes::Bytes>;
    fn http_client(&self) -> &http_client::Client;
}

fn disabled_error() -> anyhow::Error {
    anyhow!("Harness support is disabled in this local-first fork")
}

impl ServerApi {
    pub(crate) async fn get_public_api_response_for_task(
        &self,
        _task_id: &AmbientAgentTaskId,
        _path: &str,
    ) -> Result<http_client::Response> {
        Err(disabled_error())
    }

    pub(crate) async fn post_public_api_response_for_task<B>(
        &self,
        _task_id: &AmbientAgentTaskId,
        _path: &str,
        _body: &B,
    ) -> Result<http_client::Response>
    where
        B: serde::Serialize,
    {
        Err(disabled_error())
    }

    pub(crate) async fn resolve_prompt_for_task(
        &self,
        _task_id: &AmbientAgentTaskId,
        _request: ResolvePromptRequest,
    ) -> Result<ResolvedHarnessPrompt> {
        Err(disabled_error())
    }

    pub(crate) async fn fetch_transcript_for_task(
        &self,
        _task_id: &AmbientAgentTaskId,
    ) -> Result<bytes::Bytes> {
        Err(disabled_error())
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl HarnessSupportClient for ServerApi {
    async fn create_external_conversation(&self, _format: &str) -> Result<AIConversationId> {
        Err(disabled_error())
    }

    async fn get_transcript_upload_target(
        &self,
        _conversation_id: &AIConversationId,
    ) -> Result<UploadTarget> {
        Err(disabled_error())
    }

    async fn get_block_snapshot_upload_target(
        &self,
        _conversation_id: &AIConversationId,
    ) -> Result<UploadTarget> {
        Err(disabled_error())
    }

    async fn resolve_prompt(
        &self,
        _request: ResolvePromptRequest,
    ) -> Result<ResolvedHarnessPrompt> {
        Err(disabled_error())
    }

    async fn report_artifact(&self, _artifact: &Artifact) -> Result<ReportArtifactResponse> {
        Err(disabled_error())
    }

    async fn notify_user(&self, _message: &str) -> Result<()> {
        Err(disabled_error())
    }

    async fn finish_task(&self, _success: bool, _summary: &str) -> Result<()> {
        Err(disabled_error())
    }

    async fn get_snapshot_upload_targets(
        &self,
        _request: &SnapshotUploadRequest,
    ) -> Result<Vec<UploadTarget>> {
        Err(disabled_error())
    }

    async fn fetch_transcript(&self) -> Result<bytes::Bytes> {
        Err(disabled_error())
    }

    fn http_client(&self) -> &http_client::Client {
        &self.client
    }
}

pub async fn upload_to_target(
    _http_client: &http_client::Client,
    _target: &UploadTarget,
    _body: impl Into<reqwest::Body>,
) -> Result<()> {
    Ok(())
}
