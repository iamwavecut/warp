//! Passes local context to an ACP process without a hosted attachment service.
use super::protocol::{ContentBlock, EmbeddedResource, PromptCapabilities};
use crate::ai::agent::AIAgentAttachment;
use anyhow::{Result, bail};
use base64::Engine as _;
use futures::io::AsyncReadExt as _;
use std::sync::OnceLock;

pub(super) const MAX_EMBEDDED_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct AttachmentResolver {
    capabilities: OnceLock<PromptCapabilities>,
}
impl AttachmentResolver {
    pub(super) fn set_prompt_capabilities(&self, capabilities: PromptCapabilities) {
        let _ = self.capabilities.set(capabilities);
    }
    pub(super) async fn prompt_content(
        &self,
        text: String,
        attachments: Vec<AIAgentAttachment>,
    ) -> Result<Vec<ContentBlock>> {
        let mut blocks = prompt_content(text, attachments)?;
        let capabilities = self.capabilities.get().copied().unwrap_or_default();
        for block in &mut blocks {
            let ContentBlock::ResourceLink { uri, .. } = block else {
                continue;
            };
            let uri = uri.clone();
            let path = url::Url::parse(&uri)?
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("ACP attachments must be local files"))?;
            let mime_type = crate::util::image::infer_mime_type(&path, &[]);
            let image = crate::util::image::is_supported_image_mime_type(&mime_type);
            let cap = if image && capabilities.image {
                crate::util::image::MAX_IMAGE_SIZE_BYTES
            } else if !image && capabilities.embedded_context {
                MAX_EMBEDDED_TEXT_BYTES
            } else {
                continue;
            };
            let metadata = async_fs::metadata(&path).await?;
            if !metadata.is_file() || metadata.len() > cap as u64 {
                continue;
            }
            let mut bytes = Vec::new();
            async_fs::File::open(&path)
                .await?
                .take(cap as u64 + 1)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() > cap {
                continue;
            }
            if image {
                *block = ContentBlock::Image {
                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                    mime_type,
                };
            } else if let Ok(text) = String::from_utf8(bytes) {
                *block = ContentBlock::Resource {
                    resource: EmbeddedResource {
                        uri,
                        mime_type: Some(mime_type),
                        text: Some(text),
                        blob: None,
                    },
                };
            }
        }
        Ok(blocks)
    }
}
fn prompt_content(text: String, attachments: Vec<AIAgentAttachment>) -> Result<Vec<ContentBlock>> {
    let mut blocks = vec![ContentBlock::Text { text }];
    for attachment in attachments {
        let block = match attachment {
            AIAgentAttachment::PlainText(text)
            | AIAgentAttachment::DocumentContent { content: text, .. } => {
                ContentBlock::Text { text }
            }
            AIAgentAttachment::FilePathReference {
                file_path,
                file_name,
                ..
            } => {
                let path = std::fs::canonicalize(file_path)?;
                ContentBlock::ResourceLink {
                    uri: url::Url::from_file_path(path)
                        .map_err(|_| anyhow::anyhow!("attachment needs an absolute local path"))?
                        .into(),
                    name: Some(file_name),
                }
            }
            AIAgentAttachment::DriveObject { .. } => {
                bail!("hosted Drive attachments are unavailable")
            }
            local => ContentBlock::Text {
                text: serde_json::to_string(&local)?,
            },
        };
        blocks.push(block);
    }
    Ok(blocks)
}
#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
